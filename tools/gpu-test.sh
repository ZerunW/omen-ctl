#!/bin/bash
# GPU power test (HP OMEN 8D3F). Run with sudo. Needs hashcat.
# Phase A: balanced (fans BIOS auto), 90 s GPU load.
# Phase B: fans manual level 45 -> performance, wait 20 s, 90 s GPU load.
# Load = hashcat brute-forcing a made-up SHA-256 hash (no real data), OpenCL/CUDA on the RTX 5060.
# Safety: GPU > 90 C or CPU > 95 C -> stop load, balanced. End: balanced, fans auto after CPU < 60 C.
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
OUT="$DIR/../logs/gpu-test-$(date +%H%M%S).log"
mkdir -p "$DIR/../logs"
exec > >(tee "$OUT") 2>&1
U=${SUDO_USER:?run this script with sudo}
F=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
CT=$(for h in /sys/class/hwmon/hwmon*; do [ "$(cat $h/name)" = coretemp ] && echo $h; done)
DUMMY=$(printf 'omenctl-dummy-%s' "$RANDOM" | sha256sum | cut -d' ' -f1)
HC=""
cpu() { echo $(( $(cat $CT/temp1_input) / 1000 )); }
gpuq() { nvidia-smi --query-gpu=power.draw,power.limit,enforced.power.limit,clocks.sm,temperature.gpu,utilization.gpu,pstate --format=csv,noheader 2>/dev/null; }
stop_load() { [ -n "$HC" ] && kill "$HC" 2>/dev/null; pkill -u "$U" -x hashcat 2>/dev/null; HC=""; }
finish() {
    stop_load; powerprofilesctl set balanced
    for i in $(seq 60); do [ "$(cpu)" -lt 60 ] && break; sleep 5; done
    [ "$(cat $F/pwm1_enable)" != 2 ] && echo 2 > $F/pwm1_enable
    echo "== $(date +%T) balanced, fans auto. log: $OUT"
}
trap 'echo "!!! interrupted"; finish; exit 1' INT TERM
load_phase() {   # $1 label
    echo "== $(date +%T) $1: GPU load 90 s  [power.draw, power.limit, enforced.limit, sm clock, temp, util, pstate]"
    sudo -u "$U" hashcat -m 1400 -a 3 -w 4 -O --runtime=90 --potfile-disable --quiet "$DUMMY" '?a?a?a?a?a?a?a?a' >/dev/null 2>&1 &
    HC=$!
    for i in $(seq 50); do
        g=$(gpuq); gt=$(echo "$g" | cut -d, -f5 | tr -d ' ')
        echo "t=$((i*2))s cpu=$(cpu)C fan=$(cat $F/fan1_input)/$(cat $F/fan2_input) gpu=[$g]"
        { [ "${gt:-0}" -gt 90 ] || [ "$(cpu)" -gt 95 ]; } && { echo "!!! ABORT temp"; finish; exit 1; }
        kill -0 "$HC" 2>/dev/null || { echo "== hashcat ended"; break; }
        sleep 2
    done
    stop_load; sleep 15
}

echo "== $(date +%T) start: profile=$(cat /sys/firmware/acpi/platform_profile) fans=$(cat $F/fan1_input)/$(cat $F/fan2_input) gpu=[$(gpuq)]"
load_phase "A balanced"
echo "== $(date +%T) fans manual level 45"
echo 1 > $F/pwm1_enable && echo 221 > $F/pwm1 && echo 221 > $F/pwm2 || { echo "fan write failed"; finish; exit 1; }
sleep 10
echo "== $(date +%T) performance"; powerprofilesctl set performance; sleep 20
bash "$DIR/ec-read.sh"
load_phase "B performance"
finish
