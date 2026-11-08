#!/bin/bash
# Verification test: does manual fan mode (driver keep-alive every 90 s) keep the EC in
# performance (0x31, 90/130 W) beyond the 120 s window, with fans under our control?
# Run with sudo. Fans are held at level 45 (~4500 rpm, within 18..52, never max switch).
# Safety: CPU > 95 C -> abort; fans < 3000 rpm while CPU > 80 C -> abort.
# Abort/cleanup: stop load, set balanced, keep fans manual 45 until CPU < 60 C.
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
OUT="$DIR/../logs/perf-test2-$(date +%H%M%S).log"
mkdir -p "$DIR/../logs"
exec > >(tee "$OUT") 2>&1
F=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
CT=$(for h in /sys/class/hwmon/hwmon*; do [ "$(cat $h/name)" = coretemp ] && echo $h; done)
PWM=221   # level 45 of 52
pids=(); TS=""
cpu() { echo $(( $(cat $CT/temp1_input) / 1000 )); }
gpu() { nvidia-smi --query-gpu=temperature.gpu,power.draw,power.limit --format=csv,noheader 2>/dev/null; }
line() { echo "t=$1s $(date +%T) cpu=$(cpu)C fan=$(cat $F/fan1_input)/$(cat $F/fan2_input) gpu=[$(gpu)] profile=$(cat /sys/firmware/acpi/platform_profile)"; }
stop_load() { for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null; done; pids=(); }
cooldown() {
    stop_load; [ -n "$TS" ] && kill "$TS" 2>/dev/null
    powerprofilesctl set balanced
    echo "== $(date +%T) balanced, fans stay manual 45 until CPU < 60C"
    for i in $(seq 60); do c=$(cpu); line cool; [ "$c" -lt 60 ] && break; sleep 5; done
}
abort() { echo "!!! ABORT: $1"; cooldown; exit 1; }
check() {
    c=$(cpu); f1=$(cat $F/fan1_input); f2=$(cat $F/fan2_input)
    [ "$c" -gt 95 ] && abort "CPU ${c}C"
    [ "$c" -gt 80 ] && { [ "$f1" -lt 3000 ] || [ "$f2" -lt 3000 ]; } && abort "fans ${f1}/${f2} with CPU ${c}C"
    return 0
}

echo "== $(date +%T) start: $(line 0)"
echo "== set fans manual, level 45 (pwm $PWM)"
echo 1 > $F/pwm1_enable && echo $PWM > $F/pwm1 && echo $PWM > $F/pwm2 || abort "fan write failed"
for i in 1 2 3; do sleep 5; line fan; done

echo "== $(date +%T) switch to performance"
powerprofilesctl set performance; sleep 2
echo "== EC t=0"; bash "$DIR/ec-read.sh"

start=$SECONDS
echo "== idle 180 s to pass the 120 s window (keep-alive should refresh at 90 s)"
while [ $((SECONDS - start)) -lt 180 ]; do
    t=$((SECONDS - start)); line $t; check
    if [ $t -ge 100 ] && [ $t -lt 110 ]; then echo "== EC ~100s"; bash "$DIR/ec-read.sh"; fi
    sleep 10
done
echo "== EC ~180s (after window)"; bash "$DIR/ec-read.sh"

echo "== $(date +%T) all-core load 180 s"
turbostat --quiet --Summary --show Busy%,Bzy_MHz,PkgWatt,PkgTmp --interval 10 --num_iterations 18 &
TS=$!
for i in $(seq "$(nproc)"); do (while :; do :; done) & pids+=($!); done
ls_=$SECONDS
while [ $((SECONDS - ls_)) -lt 180 ]; do
    t=$((SECONDS - ls_)); line L$t; check
    if [ $t -ge 90 ] && [ $t -lt 95 ]; then echo "== EC load ~90s"; bash "$DIR/ec-read.sh"; fi
    sleep 5
done
echo "== EC end of load"; bash "$DIR/ec-read.sh"
cooldown
echo "== $(date +%T) done (fans still manual 45; profile balanced). log: $OUT"
