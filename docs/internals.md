# omenctl internals

How omenctl drives the firmware of the HP OMEN 16-am0xxx (board `8D3F`), what was measured
on the machine, and why the safety rules are what they are. The user-facing side
(install, modes, configuration) is in [USER_GUIDE.md](USER_GUIDE.md).

All findings are for **BIOS F.14** on Ubuntu 24.04 with the HWE 7.0 kernel, measured on
2026-11-08. Method names such as `GM2E` or `WMID.GSAS` refer to the decompiled DSDT and
SSDT17 of that BIOS. The tables are HP's code and are not in this repository;
[`tools/dump-acpi.sh`](../tools/dump-acpi.sh) extracts and decompiles them from your own
machine.

## Contents

1. [Layers](#1-layers)
2. [BIOS WMI commands](#2-bios-wmi-commands)
3. [EC fields](#3-ec-fields)
4. [Verified behaviour](#4-verified-behaviour)
5. [Driver patch](#5-driver-patch)
6. [Mode switching](#6-mode-switching)
7. [Fans](#7-fans)
8. [Safety layers](#8-safety-layers)
9. [Events: sleep, AC, boot](#9-events-sleep-ac-boot)
10. [Open items](#10-open-items)

## 1. Layers

```text
omenctl-gui (user)      omenctl (user)
          \                 /
      /run/omenctl.sock   (group omenctl, 0660, line-delimited JSON)
                   |
     omenctld (root, systemd Type=notify, WatchdogSec=15)
                   |   standard sysfs files only
   /sys/firmware/acpi/platform_profile                      mode
   /sys/devices/platform/hp-wmi/hwmon/hwmon*/pwm*           fans
   /sys/devices/platform/hp-wmi/omenctl_kick                kernel fan watchdog
   /sys/devices/platform/hp-wmi/omenctl_concurrent_tdp      75 W CPU+GPU target
   /sys/devices/platform/hp-wmi/omenctl_adapter_w           adapter rating (read-only)
   /sys/class/powercap/intel-rapl*                          optional PL1/PL2, GPU priority
   /sys/devices/system/cpu/cpu*/cpufreq/energy_performance_preference
                   |
     hp-wmi (patched, DKMS)  ->  BIOS WMI  ->  EC
```

Rules that hold across the whole stack:

- Every command that reaches the EC is sent by the kernel driver, and only commands
  that were observed from OMEN Gaming Hub or verified on this machine are used:
  `0x10`, `0x1A` (`0x30`/`0x31`), `0x21`/`0x22`, `0x2D`/`0x2E`/`0x2F`, and `0x29`
  with exactly two payloads (`{FF,FF,FF,75}` from omenctl and `{0,0,FF,FF}` that the
  upstream driver already sends).
- The service and the user programs only read and write sysfs files. Nothing in
  userspace opens the EC or calls WMI.
- Any failure moves the machine towards more cooling and less power.

## 2. BIOS WMI commands

Query numbers as used by `hp-wmi` (`HPWMI_GM` command type `0x20008` unless noted).

| Query | Direction | Effect on 8D3F |
|---|---|---|
| `0x1A` | write | Writes the mode byte straight into EC `0x95`. `0x30` = balanced, `0x31` = performance. `0x04` is also recognised by the BIOS but has not been tested on Linux. In `0x31` the EC raises PL1/PL2/PL4 to 90/130/200 W. Any `0x1A` switch also resets the concurrent target set by `0x29` (BIOS `DTPC`). |
| `0x10` | write | `HPWMI_FAN_COUNT_GET_QUERY` in the driver. Opens a window of about 120 s in which the EC runs the fans at the software targets from `0x2E` instead of its own curve. The 90/130 W state only holds while this window is kept alive. |
| `0x2E` | write | Fan targets in units of 100 rpm, `{cpu, gpu}`. **A target of 0 means stop, not automatic** (`GM2E` does no validation). |
| `0x2D` / `0x2F` | read | Fan speed and the BIOS fan table (12 rows, CPU and GPU columns, speeds only, no temperatures). |
| `0x29` | write | Power limits. Byte 3 is the CPU+GPU concurrent target used by NVIDIA Dynamic Boost (`NPCF.OPAT = W * 8`); 75 W, the value OMEN Gaming Hub sends in Unleashed, lifts the GPU limit from about 80 W to 115 W. Bytes 0/1 are PL2/PL1, swapped compared with the upstream struct. `0xFF` leaves a field unchanged. |
| `0x21` / `0x22` | read/write | GPU thermal profile (cTGP / PPAB / D-state), used by the upstream driver for the platform profiles. |
| `0x0F` | read (`HPWMI_READ`) | Adapter status. `WMID.GSAS` only reads EC fields and returns 4 bytes: `[0]` status, `[1]` `0xAE`, `[2]` 0, `[3]` rated power in units of 5 W (0 = no adapter, `0xFF` = unknown). The BIOS maps the EC adapter id to 9, 13, 18, 24, 27, 30, 36, 40, 46, 56, 66, 72 (barrel, 45 to 360 W) and 1, 3, 6, 9, 13, 18, 20, 28 (USB-C, 5 to 140 W). |
| `0x27` | write | Firmware "max fan" switch. **Never used**: a manual 100 % fan freeze on 8D3F is reported in [OmenMon-Reborn #137](https://github.com/metamountain/OmenMon-Reborn-8BCA/issues/137). omenctl's "max" is fan table level 52 through `0x2E`. |

The BIOS does almost no parameter validation, which is why only values seen from
OMEN Gaming Hub are ever sent.

## 3. EC fields

Offsets from `Field (ERAM ...)` in the DSDT. [`tools/ec-read.sh`](../tools/ec-read.sh)
prints them read-only (it loads `ec_sys` without `write_support`).

| Offset | DSDT name | Meaning |
|---|---|---|
| `0x95` | `HPCM` | Mode byte (`0x30` = 48 balanced, `0x31` = 49 performance) |
| `0x62` bit 0 | `OMCC` | Software fan control heartbeat |
| `0x63` | | Countdown of the `0x10` window (seconds) |
| `0xEC` bit 2 | `FFFS` | Firmware max-fan switch (`0x27`) |
| `0x37` / `0x38` / `0x39` | | PL1 / PL2 / PL4 requested by software (W) |
| `0x3A` / `0x3B` / `0x3C` | | PL1 / PL2 / PL4 in effect (W) |
| `0x34` / `0x35` | | Fan targets written by `0x2E` |
| `0x90` / `0x91` | `NVDO` / `ECDO` | GPU D-state |

## 4. Verified behaviour

Measured with the scripts in [`tools/`](../tools/) (each one runs with `sudo`, holds
the fans at a safe manual level and has temperature aborts; they write logs to
`logs/`, which is not tracked).

| Finding | How it was measured |
|---|---|
| `0x1A` = `0x31` gives PL1/PL2/PL4 = 90/130/200 W in EC `0x3A`-`0x3C` | `perf-test.sh`, `perf-test2.sh`: EC snapshot right after the switch |
| The state is tied to the `0x10` window; inside it the EC does not adjust the fans to temperature | `perf-test.sh`: fans stayed at 2400/2100 rpm while the CPU went from 56 to 91 °C under load (test aborted at 91 °C) |
| In manual fan mode the upstream driver refreshes the window every 90 s and the EC keeps `0x31` and 90/130 W indefinitely | `perf-test2.sh`: 180 s idle plus 180 s all-core load; EC still at 90/130/200 W, countdown at `0x63` refreshed |
| Sustained all-core load in performance: about 130 W for ~50 s, then 90 W at about 3.25 GHz busy clock, CPU 74-75 °C, fans at 4400 rpm | `perf-test2.sh` (turbostat) |
| P-core clocks in performance (EPP `performance`): 1 thread 4.94 GHz (50 W), 4 threads 4.80 GHz (96 W), 8 threads on 8 P-cores 4.41 GHz (130 W), 24 threads P 3.78 / E 3.03 GHz | `pcore-test.sh` |
| GPU power limit under a GPU-only load: balanced about 82-83 W; performance alone about 92-95 W; performance plus `0x29` byte 3 = 75 gives a constant 115 W limit | `gpu-test.sh`, `gpu-test75.sh` (`nvidia-smi` enforced limit) |
| With fans in "auto", the upstream driver writes `0x2E {0,0}` and the fans stop for about 120 s | observed during testing; fixed by the smooth handover in the patch |
| With the smooth handover, fans never read 0 rpm across a driver reload, a manual-to-auto switch or a profile change | `handover-test.sh`: 0 samples at 0 rpm over three 130 s phases |
| Kernel watchdog: with fans manual and no kick for 15 s the driver logs `omenctl watchdog: no kick for 15s, fans forced to max level` and the fans ramp from 2500 to 5200 rpm within about 10 s; kicking again clears `tripped` | `wd-test.sh` |
| Fan pwm mapping: pwm 128 gives about 2500 rpm, pwm 221 about 4400 rpm, pwm 255 5200 rpm (the driver's double integer conversion loses one level, e.g. pwm 221 lands on level 44, not 45) | `perf-test2.sh`, `wd-test.sh`; emulated in `curve::driver_written_level` |

Combined CPU + GPU load and the `gpu_priority_pl1` behaviour were measured later with
`tools/stress-test.sh`, which runs as a normal user with omenctld in charge; the results
are in the table below and in [CHANGELOG.md](../CHANGELOG.md) (0.2.7, 0.2.8).

### Measured results

HP OMEN 16-am0xxx, Ubuntu 24.04, HWE kernel 7.0, BIOS F.14. Stock Linux sends no
performance commands to the EC, so the CPU stays at PL1 45 W / PL2 56 W (35 / 45 W in
balanced) and the GPU at about 80 W.

| | stock Linux | omenctl Unleashed |
|---|---|---|
| EC CPU limits PL1 / PL2 / PL4 | 45 / 56 / 90 W | 90 / 130 / 200 W |
| CPU, 1 thread | – | 4.94 GHz |
| CPU, 8 threads (one per P-core) | – | 4.41 GHz @ 130 W |
| CPU, all 24 threads, first ~50 s | – | P 4.0–4.6 / E 3.3–3.7 GHz, 130 W |
| CPU, all 24 threads sustained | ~2.45 GHz @ ~45–56 W | P 3.5–3.78 / E 2.8–3.03 GHz, 90 W |
| GPU power limit | 80–83 W | 115 W |
| CPU + GPU at full load, sustained | – | CPU 70 W + GPU limit 105 W (GPU priority) |

Clock ranges depend on the load.

The EC also behaves in ways that make a plain "switch to performance" unsafe on its own:
the limits only hold while software keeps the fans under manual control (the `0x10`
window), a fan target of 0 means stop, and handing the fans back to the BIOS can pause
them for about two minutes. These are the reasons for the safety layers in section 8.

## 5. Driver patch

`hpwmi-upstream/src/hp-wmi.c` is mainline `hp-wmi.c` at commit `e931e54edb0f` plus
[`backport.patch`](../hpwmi-upstream/backport.patch). Provenance is in
[hpwmi-upstream/README.md](../hpwmi-upstream/README.md). The patch:

1. Defines `ACPI_AC_CLASS` when missing, so the driver builds on Ubuntu's 7.0 HWE
   kernel (the define moved to a shared header in v7.1).
2. Adds `omenctl_concurrent_tdp` (write-only, 8D3F only). Accepts only `75` and sends
   `0x29` with PL1/PL2/PL4 = `0xFF` (unchanged) and the concurrent target = 75.
3. Adds a kernel fan watchdog. After the first write to `omenctl_kick` the watchdog is
   armed; a delayed work item checks every 5 s, and if the fans are manual and no kick
   arrived for `omenctl_wd_timeout` seconds (module parameter, default 15, clamped to
   5-60) it sets both pwm values to 255 (top table level), logs at `KERN_CRIT` and sets
   `tripped`. Reading `omenctl_kick` shows `armed`, `tripped`, `since_kick`, `timeout`
   and the pwm mode.
4. Smooth fan handover on 8D3F. Handing the fans back to the BIOS writes the current
   measured speed (clamped to the fan table, never 0) instead of `{0,0}`, and lets the
   120 s window expire so the EC resumes its own curve. The same hold is applied after a
   platform profile change while the fans are in BIOS-auto mode, because that path also
   sends `0x10`.
5. Adds `omenctl_adapter_w` (read-only, 8D3F only): rated adapter power from query
   `0x0F`, printed in watts; `-ENODATA` without an adapter.

Every addition checks `DMI_BOARD_NAME == "8D3F"` and is a no-op elsewhere. The DKMS
config ([`packaging/dkms/dkms.conf`](../packaging/dkms/dkms.conf)) builds the module
only for 7.x kernels and installs it to `updates/dkms/`, which `depmod` prefers over
the stock module.

## 6. Mode switching

| Mode | Platform profile | EC | Fans | CPU+GPU target |
|---|---|---|---|---|
| Power Saver | `low-power` | `0x30`, cTGP/PPAB off | any | default |
| Balanced | `balanced` | `0x30`, PPAB on | any | default |
| Unleashed | `performance` | `0x31`, 90/130/200 W | software only | 75 W, GPU limit 115 W |

A separate "performance" mode was dropped: it would differ from Unleashed only by the
75 W target, and that budget is only used when the GPU asks for it.

The controller (`omenctl-core/src/controller.rs`) applies a mode in this order:

1. In Unleashed (or whenever protection needs software fans), kick the watchdog,
   switch the fans to manual and write the wanted levels first, so the 120 s window
   opened by the profile switch never runs with stale or zero fan targets.
2. Write the platform profile.
3. In Unleashed, write `75` to `omenctl_concurrent_tdp`.
4. Optional RAPL limits for the mode, then the EPP (Unleashed only) and GPU priority.
5. From the next tick the fans follow the selected fan mode.

BIOS-auto fans are refused in Unleashed, because the EC drops the 90/130 W state once
the window is no longer refreshed. When the fans go back to the BIOS, the EC takes over
after the window expires; the safety script only does this once the CPU is below 60 °C.

## 7. Fans

- Levels are in units of 100 rpm, from 18 (1800 rpm) to 52 (5200 rpm). The driver
  clamps to this range and never writes 0.
- Curve: piecewise linear, rounded up, clamped to [18, 52]. The input is the hotter of
  CPU and GPU, averaged over `average_s` (default 5 s). Rises are followed at once;
  after the last rise the level is held for `hold_s` (15 s), then falls by at most
  `fall_per_s` (0.5 levels, i.e. 50 rpm, per second).
- Default curves are the BIOS fan table read with `0x2F` (CPU column 18 to 52, GPU
  column raised to at least 18). The table has speeds but no temperatures; the
  temperatures assigned to its 12 rows (40 to 85 °C, with 85 °C and above at level 52)
  are provisional until calibrated against BIOS-auto behaviour.
- Fan writes go through pwm, so `curve::level_to_pwm` picks the smallest pwm that
  makes the driver land on at least the wanted level.

## 8. Safety layers

1. **Kernel watchdog** (driver): the service writes `omenctl_kick` every tick (1 s)
   while it is really driving the fans. If fan writes fail, it deliberately stops
   kicking so the driver forces the fans to maximum. Timeout 15 s, checked every 5 s.
2. **systemd watchdog**: `WatchdogSec=15`; a hung service is killed and restarted.
   `ExecStopPost=/usr/lib/omenctl/omenctl-safe-exit` runs after every stop; after an
   abnormal one it sets the fans to the top level and the profile to balanced, and
   hands the fans back to the BIOS only once the CPU is below 60 °C.
3. **Fan check**: measured speed below 70 % of the target for 10 s (after a ramp grace
   period) counts as a fault: fans to maximum, Unleashed is left, an alert is shown.
4. **Temperature**: at 92 °C both fans go to maximum in every fan mode; 10 s at 95 °C
   leaves Unleashed until 30 s below 80 °C. A missing CPU temperature is treated as hot.

Below these sit the CPU's and GPU's own thermal throttling and shutdown.

## 9. Events: sleep, AC, boot

- **Resume**: the EC can lose its state while asleep. The service detects resume from
  the jump between `CLOCK_BOOTTIME` and `CLOCK_MONOTONIC` and re-runs the full mode
  switch, rewriting fan values even if they look unchanged.
- **AC plug/unplug**: the upstream driver sends `0x29 {0,0,FF,FF}` on a power source
  change in performance mode, which can clear the 75 W target. Without a power-source
  rule, the service writes 75 again in Unleashed.
- **Boot**: loading the driver sends `0x10`, so the fans may pause for about two minutes
  while idle; the service takes over when it starts. `omenctl-check` waits up to 30 s for
  the patched driver, because udev loads `hp-wmi` asynchronously.
- **nvidia-powerd**: required for the GPU side of the concurrent target. The package
  enables it (or its own `omenctl-nvidia-powerd.service` when the NVIDIA packages ship
  no unit) and installs a root-only D-Bus policy for it, based on
  [omen_max_gpu_unlock](https://github.com/WangSteve123/omen_max_gpu_unlock) with the
  non-root send rule removed.
- **GPU sleep**: NVML is only queried while the dGPU is already runtime-active, at most
  every 5 s, on its own thread, and shut down after each read, so the service never keeps
  the GPU awake or blocks the control loop while the GPU suspends.

## 10. Open items

- Calibrate the temperatures of the BIOS fan table rows (see
  [HARDWARE_TEST.md](HARDWARE_TEST.md), "Later").
- Mode byte `0x04`.
- GPU MUX switching (iGPU / dGPU / Advanced Optimus), with a recovery path that works
  without a desktop.
- Measured CPU limits for Power Saver and Balanced (currently firmware defaults; optional
  `[rapl]` values per mode exist in the config).
