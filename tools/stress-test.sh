#!/bin/bash
# Stress test with omenctld in charge: CPU alone, GPU alone, then both.
# Runs as a normal user (member of the omenctl group); it only creates load and
# reads values. Fans, power mode and overheat protection stay with omenctld.
#
#   tools/stress-test.sh [log file] [cpu_s] [gpu_s] [both_s]
#
# CPU load: openssl SHA-256 on every logical CPU. GPU load: hashcat brute-forcing
# a made-up SHA-256 hash (CUDA). Samples every 2 s: mode, temperatures, CPU/GPU
# power (from omenctld), GPU limit/clock/state (nvidia-smi), P/E-core clocks, fans.
# Stops the load early if the CPU reaches 98 C or the GPU 90 C.
set -u
OUT=${1:-./stress-$(date +%H%M%S).log}
CPU_S=${2:-120}; GPU_S=${3:-120}; BOTH_S=${4:-180}; REST_S=45
exec > >(tee "$OUT") 2>&1

PCORES=$(cat /sys/devices/cpu_core/cpus 2>/dev/null)
ECORES=$(cat /sys/devices/cpu_atom/cpus 2>/dev/null)
DUMMY=$(printf 'omenctl-dummy-%s' "$RANDOM" | sha256sum | cut -d' ' -f1)
pids=()

expand() { local r out=""; for r in ${1//,/ }; do out+=" $(seq ${r%-*} ${r#*-})"; done; echo $out; }
avg_mhz() { local c s=0 n=0; for c in $(expand "$1"); do s=$((s + $(cat /sys/devices/system/cpu/cpu$c/cpufreq/scaling_cur_freq))); n=$((n+1)); done; echo $((s / n / 1000)); }
gpuq() { nvidia-smi --query-gpu=enforced.power.limit,clocks.sm,utilization.gpu,pstate --format=csv,noheader,nounits 2>/dev/null | tr -d ' '; }

stop_load() {
	for p in "${pids[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
	pkill -u "$(id -u)" -x openssl 2>/dev/null; pkill -u "$(id -u)" -x hashcat 2>/dev/null
	pids=()
}
trap 'echo "!!! interrupted"; stop_load; exit 1' INT TERM

cpu_load() { timeout "$1" openssl speed -multi "$(nproc)" -seconds 3600 -evp sha256 >/dev/null 2>&1 & pids+=($!); }
gpu_load() { hashcat -m 1400 -a 3 -w 4 -O --runtime="$1" --potfile-disable --quiet "$DUMMY" '?a?a?a?a?a?a?a?a?a' >/dev/null 2>&1 & pids+=($!); }

# one sample: t mode cpuC gpuC cpuW gpuW gpuLimit smMHz util pstate Pmhz Emhz fan1 fan2 alerts
sample() {
	local s g
	s=$(omenctl status --json 2>/dev/null) || { echo "t=$1 omenctl status failed"; return; }
	g=$(gpuq)
	printf '%4s %-10s %5s %5s %6s %6s  %-26s P%5s E%5s  %4s/%-4s %s\n' "$1" \
		"$(jq -r .effective_mode <<<"$s")" \
		"$(jq -r '.cpu_temp // "-"' <<<"$s")" "$(jq -r '.gpu_temp // "-"' <<<"$s")" \
		"$(jq -r '(.cpu_power_w // 0)*10|round/10' <<<"$s")" "$(jq -r '(.gpu_power_w // 0)*10|round/10' <<<"$s")" \
		"$g" "$(avg_mhz "$PCORES")" "$(avg_mhz "$ECORES")" \
		"$(jq -r '.fan_rpm[0]' <<<"$s")" "$(jq -r '.fan_rpm[1]' <<<"$s")" \
		"$(jq -r '[.alerts[]?]|join("; ")' <<<"$s")"
	local ct gt
	ct=$(jq -r '.cpu_temp // 0 | floor' <<<"$s"); gt=$(jq -r '.gpu_temp // 0 | floor' <<<"$s")
	if [ "$ct" -ge 98 ] || [ "$gt" -ge 90 ]; then echo "!!! limit reached (CPU ${ct} C, GPU ${gt} C): stopping load"; stop_load; return 1; fi
}

phase() { # name seconds loads...
	local name=$1 secs=$2; shift 2
	echo
	echo "== $(date +%T) $name, $secs s"
	echo "   t mode        cpuC  gpuC   cpuW   gpuW  gpu limit,MHz,util,state   P-core E-core  fans      alerts"
	for l in "$@"; do $l "$secs"; done
	local t=0
	while [ $t -le "$secs" ]; do sample "$t" || break; sleep 2; t=$((t + 2)); done
	stop_load
	echo "== $(date +%T) rest $REST_S s"
	t=0; while [ $t -le $REST_S ]; do [ $((t % 10)) = 0 ] && sample "r$t"; sleep 2; t=$((t + 2)); done
}

s=$(omenctl status --json) || { echo "omenctld not reachable"; exit 1; }
echo "omenctl stress test $(date '+%F %T'), kernel $(uname -r), BIOS $(cat /sys/class/dmi/id/bios_version)"
echo "mode $(jq -r .mode <<<"$s"), AC $(jq -r .ac_online <<<"$s"), $(nproc) CPUs (P $PCORES, E $ECORES)"
echo "GPU: $(nvidia-smi --query-gpu=name,driver_version,power.max_limit --format=csv,noheader)"
[ "$(jq -r .ac_online <<<"$s")" = true ] || echo "WARNING: on battery, results are not comparable"

phase "CPU only" "$CPU_S" cpu_load
phase "GPU only" "$GPU_S" gpu_load
phase "CPU + GPU" "$BOTH_S" cpu_load gpu_load
echo
echo "== $(date +%T) done. log: $OUT"
