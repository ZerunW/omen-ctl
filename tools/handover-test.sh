#!/bin/bash
# Smooth fan handover test (HP OMEN 8D3F). Run with sudo. Idle, no performance mode.
# A) reload driver -> fans must never read 0 for 130 s
# B) manual pwm 128 (~2500 rpm) 15 s -> back to auto -> fans held, never 0, for 130 s
# C) with fans auto: balanced -> low-power -> balanced -> fans never 0 for 130 s
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$DIR/.."
KO="$ROOT/hpwmi-upstream/src/hp-wmi.ko"
OUT="$ROOT/logs/handover-test-$(date +%H%M%S).log"
mkdir -p "$ROOT/logs"
exec > >(tee "$OUT") 2>&1
CT=$(for h in /sys/class/hwmon/hwmon*; do [ "$(cat $h/name)" = coretemp ] && echo $h; done)
zeros=0
watch_fans() {   # $1 label, $2 seconds
    local F; F=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
    for i in $(seq $(( $2 / 5 ))); do
        f1=$(cat $F/fan1_input); f2=$(cat $F/fan2_input)
        echo "$(date +%T) $1 fans=$f1/$f2 cpu=$(( $(cat $CT/temp1_input)/1000 ))C profile=$(cat /sys/firmware/acpi/platform_profile)"
        { [ "$f1" = 0 ] || [ "$f2" = 0 ]; } && zeros=$((zeros+1))
        sleep 5
    done
}

echo "== $(date +%T) A: sign + reload driver"
kmodsign sha512 /var/lib/shim-signed/mok/MOK.priv /var/lib/shim-signed/mok/MOK.der "$KO" || exit 1
rmmod hp_wmi; modprobe sparse_keymap; insmod "$KO" || { echo "insmod failed, restoring stock"; modprobe hp_wmi; exit 1; }
sleep 1
echo "srcversion=$(cat /sys/module/hp_wmi/srcversion)"
watch_fans A 130

F=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
echo "== $(date +%T) B: manual pwm 128 for 15 s, then auto"
echo 1 > $F/pwm1_enable && echo 128 > $F/pwm1 && echo 128 > $F/pwm2
watch_fans B-manual 15
echo 2 > $F/pwm1_enable
watch_fans B-auto 130

echo "== $(date +%T) C: fans auto, balanced -> low-power -> balanced"
powerprofilesctl set power-saver; sleep 3; powerprofilesctl set balanced
watch_fans C 130

echo "== $(date +%T) done. samples with a fan at 0: $zeros. log: $OUT"
