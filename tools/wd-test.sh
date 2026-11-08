#!/bin/bash
# Kernel fan-watchdog test (HP OMEN 8D3F). Run with sudo. Stays in balanced mode, idle.
# 1) sign + load patched hp-wmi  2) fans manual ~2600 rpm (pwm 128 = level 26)
# 3) kick every 2 s for 20 s (should NOT trip)  4) stop kicking 30 s (should trip -> fans ~5200)
# 5) kick again (tripped clears), set pwm 128 again  6) fans back to BIOS auto (pause ~2 min, idle)
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="$DIR/.."
KO="$ROOT/hpwmi-upstream/src/hp-wmi.ko"
OUT="$ROOT/logs/wd-test-$(date +%H%M%S).log"
mkdir -p "$ROOT/logs"
exec > >(tee "$OUT") 2>&1
P=/sys/devices/platform/hp-wmi

echo "== $(date +%T) sign + load"
kmodsign sha512 /var/lib/shim-signed/mok/MOK.priv /var/lib/shim-signed/mok/MOK.der "$KO" || exit 1
rmmod hp_wmi; modprobe sparse_keymap; insmod "$KO" || { echo "insmod failed, restoring stock"; modprobe hp_wmi; exit 1; }
sleep 2
F=$(ls -d $P/hwmon/hwmon*)
st() { echo "$(date +%T) $1 fans=$(cat $F/fan1_input)/$(cat $F/fan2_input) | $(cat $P/omenctl_kick)"; }
echo "srcversion=$(cat /sys/module/hp_wmi/srcversion) profile=$(cat /sys/firmware/acpi/platform_profile)"
st "loaded"

echo "== fans manual, pwm 128"
echo 1 > $F/pwm1_enable && echo 128 > $F/pwm1 && echo 128 > $F/pwm2
echo 1 > $P/omenctl_kick
for i in $(seq 10); do sleep 2; echo 1 > $P/omenctl_kick; st "kicking"; done

echo "== stop kicking"
for i in $(seq 15); do sleep 2; st "silent"; done

echo "== kick again, pwm back to 128"
echo 1 > $P/omenctl_kick; echo 128 > $F/pwm1; echo 128 > $F/pwm2
for i in 1 2 3 4; do sleep 2; echo 1 > $P/omenctl_kick; st "recovered"; done

echo "== kernel log"
journalctl -k -b --no-pager --since "-3min" | grep -i omenctl

echo "== fans back to BIOS auto (pause ~2 min while idle)"
echo 2 > $F/pwm1_enable
st "auto"
echo "== $(date +%T) done. log: $OUT"
