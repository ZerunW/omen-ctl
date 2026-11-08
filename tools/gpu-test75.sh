#!/bin/bash
# Concurrent-TDP 75 W test (HP OMEN 8D3F). Run with sudo.
# 1) sign + reload the patched hp-wmi (adds write-only omenctl_concurrent_tdp, accepts only 75)
# 2) fans manual level 45 immediately, performance, write 75
# 3) GPU-only load 90 s, then GPU + all-CPU load 90 s
# Safety: GPU > 90 C or CPU > 95 C -> stop, balanced. End: balanced (BIOS resets OPAT), fans auto after CPU < 60 C.
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$DIR/.."
KO="$ROOT/hpwmi-upstream/src/hp-wmi.ko"
OUT="$ROOT/logs/gpu-test75-$(date +%H%M%S).log"
mkdir -p "$ROOT/logs"
exec > >(tee "$OUT") 2>&1
U=${SUDO_USER:?run this script with sudo}
DUMMY=$(printf 'omenctl-dummy-%s' "$RANDOM" | sha256sum | cut -d' ' -f1)
HC=""; pids=()

echo "== $(date +%T) sign + reload driver"
kmodsign sha512 /var/lib/shim-signed/mok/MOK.priv /var/lib/shim-signed/mok/MOK.der "$KO" || exit 1
rmmod hp_wmi; modprobe sparse_keymap; insmod "$KO" || { echo "insmod failed, restoring stock"; modprobe hp_wmi; exit 1; }
sleep 2
F=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
CT=$(for h in /sys/class/hwmon/hwmon*; do [ "$(cat $h/name)" = coretemp ] && echo $h; done)
echo "loaded srcversion=$(cat /sys/module/hp_wmi/srcversion)"
echo 1 > $F/pwm1_enable && echo 221 > $F/pwm1 && echo 221 > $F/pwm2 || { echo "fan write failed"; exit 1; }

cpu() { echo $(( $(cat $CT/temp1_input) / 1000 )); }
gpuq() { nvidia-smi --query-gpu=power.draw,enforced.power.limit,clocks.sm,temperature.gpu,utilization.gpu --format=csv,noheader 2>/dev/null; }
stop_load() { [ -n "$HC" ] && kill "$HC" 2>/dev/null; pkill -u "$U" -x hashcat 2>/dev/null; HC=""
              for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null; done; pids=(); }
finish() {
    stop_load; powerprofilesctl set balanced
    for i in $(seq 60); do [ "$(cpu)" -lt 60 ] && break; sleep 5; done
    echo 2 > $F/pwm1_enable
    echo "== $(date +%T) balanced, fans auto. log: $OUT"
}
trap 'echo "!!! interrupted"; finish; exit 1' INT TERM
phase() {   # $1 label, $2 = cpu|nocpu
    echo "== $(date +%T) $1  [gpu draw, enforced limit, sm clock, temp, util]"
    sudo -u "$U" hashcat -m 1400 -a 3 -w 4 -O --runtime=90 --potfile-disable --quiet "$DUMMY" '?a?a?a?a?a?a?a?a' >/dev/null 2>&1 &
    HC=$!
    if [ "$2" = cpu ]; then for i in $(seq 23); do (while :; do :; done) & pids+=($!); done; fi
    sleep 4
    turbostat --quiet --Summary --show PkgWatt,Bzy_MHz --interval 15 --num_iterations 5 2>/dev/null | sed 's/^/  cpu-turbostat: /' &
    for i in $(seq 43); do
        g=$(gpuq); gt=$(echo "$g" | cut -d, -f4 | tr -d ' ')
        echo "t=$((i*2))s cpu=$(cpu)C fan=$(cat $F/fan1_input)/$(cat $F/fan2_input) gpu=[$g]"
        { [ "${gt:-0}" -gt 90 ] || [ "$(cpu)" -gt 95 ]; } && { echo "!!! ABORT temp"; finish; exit 1; }
        kill -0 "$HC" 2>/dev/null || break
        sleep 2
    done
    stop_load; wait 2>/dev/null; sleep 15
}

sleep 8
echo "== $(date +%T) performance"; powerprofilesctl set performance; sleep 15
bash "$DIR/ec-read.sh"
echo "== write concurrent TDP 75"
echo 75 > /sys/devices/platform/hp-wmi/omenctl_concurrent_tdp && echo "ok" || echo "write failed"
sleep 3
phase "GPU only, perf + 75W" nocpu
phase "GPU + CPU, perf + 75W" cpu
finish
