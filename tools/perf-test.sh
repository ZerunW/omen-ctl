#!/bin/bash
# One-shot performance-mode test for HP OMEN 8D3F. Run with sudo.
# - re-applies the performance profile (balanced -> performance) at idle
# - reads EC immediately, at ~60 s and at ~130 s (after the 120 s EC window)
# - runs an all-core busy load from t=10 s to t=190 s while turbostat logs every 10 s
# - safety: CPU > 90 C, or both fans at 0 while CPU > 80 C -> stop load, set balanced
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
OUT="$DIR/../logs/perf-test-$(date +%H%M%S).log"
mkdir -p "$DIR/../logs"
exec > >(tee "$OUT") 2>&1
U=${SUDO_USER:?run this script with sudo}
pp() { sudo -u "$U" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u "$U")/bus" powerprofilesctl set "$1"; }
F=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
CT=$(for h in /sys/class/hwmon/hwmon*; do [ "$(cat $h/name)" = coretemp ] && echo $h; done)
pids=()
stop_load() { for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null; done; pids=(); }
abort() { echo "!!! ABORT: $1"; stop_load; pp balanced; kill "$TS" 2>/dev/null; exit 1; }

echo "== $(date +%T) re-apply profile: balanced -> performance"
pp balanced; sleep 2; pp performance; sleep 2
echo "== $(date +%T) EC right after switch"; bash "$DIR/ec-read.sh"

turbostat --quiet --Summary --show Busy%,Bzy_MHz,PkgWatt,PkgTmp --interval 10 --num_iterations 20 &
TS=$!
start=$SECONDS
loaded=0
while [ $((SECONDS - start)) -lt 200 ]; do
    t=$((SECONDS - start))
    if [ $loaded = 0 ] && [ $t -ge 10 ]; then
        for i in $(seq "$(nproc)"); do (while :; do :; done) & pids+=($!); done
        loaded=1; echo "== $(date +%T) load started"
    fi
    if [ $loaded = 1 ] && [ $t -ge 190 ]; then stop_load; loaded=2; echo "== $(date +%T) load stopped"; fi
    cpu=$(( $(cat $CT/temp1_input) / 1000 )); f1=$(cat $F/fan1_input); f2=$(cat $F/fan2_input)
    echo "t=${t}s $(date +%T) cpu=${cpu}C fan=${f1}/${f2}"
    [ "$cpu" -gt 90 ] && abort "CPU ${cpu}C"
    [ "$cpu" -gt 80 ] && [ "$f1" = 0 ] && [ "$f2" = 0 ] && abort "fans at 0 with CPU ${cpu}C"
    if [ $t -ge 60 ] && [ $t -lt 65 ]; then echo "== EC at ~60s"; bash "$DIR/ec-read.sh"; fi
    if [ $t -ge 130 ] && [ $t -lt 135 ]; then echo "== EC at ~130s"; bash "$DIR/ec-read.sh"; fi
    sleep 5
done
stop_load; wait "$TS" 2>/dev/null
echo "== $(date +%T) done, log: $OUT"
