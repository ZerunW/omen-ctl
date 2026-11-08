#!/bin/bash
# P-core frequency test in performance mode (HP OMEN 8D3F). Run with sudo.
# Fans: manual level 45 during the test (so the 0x10 window never freezes them),
#       back to BIOS auto at the end only after CPU < 60 C.
# Phases (30 s each): 1, 4, 8 threads pinned to distinct physical P-cores; then all 24 threads.
# Safety: CPU > 95 C -> abort.
set -u
DIR="$(cd "$(dirname "$0")" && pwd)"
OUT="$DIR/../logs/pcore-test-$(date +%H%M%S).log"
mkdir -p "$DIR/../logs"
exec > >(tee "$OUT") 2>&1
F=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
CT=$(for h in /sys/class/hwmon/hwmon*; do [ "$(cat $h/name)" = coretemp ] && echo $h; done)
PCORES=(0 2 4 6 8 10 12 14)   # one logical CPU per physical P-core
pids=()
cpu() { echo $(( $(cat $CT/temp1_input) / 1000 )); }
stop_load() { for p in "${pids[@]:-}"; do kill "$p" 2>/dev/null; done; pids=(); }
finish() {
    stop_load; powerprofilesctl set balanced
    for i in $(seq 60); do [ "$(cpu)" -lt 60 ] && break; sleep 5; done
    echo 2 > $F/pwm1_enable
    echo "== $(date +%T) balanced, fans back to auto (they pause ~2 min). log: $OUT"
}
trap 'echo "!!! interrupted"; finish; exit 1' INT TERM

measure() {   # $1 = label; runs turbostat once over 20 s and summarizes P/E cores
    local tsout
    tsout=$(turbostat --quiet --show CPU,Busy%,Bzy_MHz,PkgWatt,PkgTmp --interval 20 --num_iterations 1 2>/dev/null)
    echo "$tsout" | awk -v L="$1" '
      NR==1 { for (i=1;i<=NF;i++) col[$i]=i; next }
      $col["CPU"]=="-" { pw=$col["PkgWatt"]; pt=$col["PkgTmp"]; next }
      { c=$col["CPU"]; b=$col["Busy%"]; m=$col["Bzy_MHz"];
        if (b>90) { if (c<16) {pn++; ps+=m; if(m>pm)pm=m} else {en++; es+=m; if(m>em)em=m} } }
      END { printf "%-10s P-busy=%d avg=%s max=%s MHz | E-busy=%d avg=%s MHz | pkg=%sW %sC\n",
            L, pn, (pn?int(ps/pn):"-"), (pn?pm:"-"), en, (en?int(es/en):"-"), pw, pt }'
    [ "$(cpu)" -gt 95 ] && { echo "!!! ABORT CPU $(cpu)C"; finish; exit 1; }
}
run_phase() {   # $1 label, rest = cpus to pin (or "all")
    local label=$1; shift
    if [ "$1" = all ]; then
        for i in $(seq "$(nproc)"); do (while :; do :; done) & pids+=($!); done
    else
        for c in "$@"; do taskset -c "$c" bash -c 'while :; do :; done' & pids+=($!); done
    fi
    sleep 5; measure "$label"; stop_load; sleep 10
}

echo "== $(date +%T) fans manual level 45"
echo 1 > $F/pwm1_enable && echo 221 > $F/pwm1 && echo 221 > $F/pwm2 || { echo "fan write failed"; exit 1; }
sleep 10
echo "== $(date +%T) performance"; powerprofilesctl set performance; sleep 3
bash "$DIR/ec-read.sh"
run_phase "1 thread"  "${PCORES[0]}"
run_phase "4 threads" "${PCORES[@]:0:4}"
run_phase "8 threads" "${PCORES[@]}"
run_phase "24 thr"    all
bash "$DIR/ec-read.sh"
finish
