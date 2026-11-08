# omenctl

Performance and fan control for the **HP OMEN 16-am0xxx (board 8D3F)** on Linux.

On Linux this laptop's embedded controller (EC) keeps the CPU at PL1 45 W / PL2 56 W
(35/45 W in balanced) and the RTX 5060 at ~80 W, because nothing sends the firmware
commands that OMEN Gaming Hub sends on Windows. omenctl reproduces those commands through
the kernel's `hp-wmi` driver and adds the fan control the firmware expects in that state.

**Status: work in progress.** The kernel driver changes are tested on hardware; the daemon,
CLI and GUI are being written. Use at your own risk.

## Measured results (Ubuntu 24.04, HWE kernel 7.0, BIOS F.14)

| | stock Linux | omenctl "Unleashed" |
|---|---|---|
| EC CPU limits PL1 / PL2 / PL4 | 45 / 56 / 90 W | 90 / 130 / 200 W |
| CPU, 1 thread | – | 4.94 GHz |
| CPU, 8 threads (one per P-core) | – | 4.41 GHz @ 130 W |
| CPU, all 24 threads sustained | ~2.45 GHz @ ~45–56 W | P 3.78 / E 3.03 GHz, 90 W |
| GPU power limit | 80–83 W | 115 W |

Raw logs are in [`logs/`](logs/).

## How it works (8D3F specifics)

Findings from the decompiled BIOS (`acpi/dsdt.dsl`, `acpi/ssdt17.dsl`) and EC readings:

- WMI `0x20008`/`0x1A` writes the mode byte straight into EC `0x95` (0x30 balanced,
  0x31 performance, 0x04 is also recognised). In 0x31 the EC raises PL1/PL2/PL4 to 90/130/200 W.
- `0x10` opens a ~120 s window in which the EC runs the fans at the software targets
  (`0x2E`, EC `0x34/0x35`) instead of its own curve. The performance limits only hold while
  that window is kept alive, so the fans must be driven by software (this is what OMEN Gaming
  Hub does). The upstream driver refreshes the window every 90 s in manual fan mode.
- A `0x2E` target of 0 means **stop**, not automatic, on this EC.
- `0x29` byte 3 sets the CPU+GPU concurrent target used by NVIDIA Dynamic Boost
  (`NPCF.OPAT = W * 8`); 75 W (the value OMEN Gaming Hub uses) lifts the GPU limit to 115 W.
  Bytes 0/1 are PL2/PL1 (order is swapped versus the upstream struct).
- The BIOS does almost no parameter validation, so only values observed from
  OMEN Gaming Hub are ever sent.

## Repository layout

| Path | Content |
|---|---|
| `PLAN.md` | design and implementation plan (Chinese) |
| `hpwmi-upstream/` | upstream `hp-wmi.c` (commit `e931e54edb0f`) + `backport.patch` |
| `tools/` | read-only EC snapshot script and hardware test scripts |
| `logs/` | test logs |
| `acpi/` | decompiled ACPI tables of board 8D3F, BIOS F.14 |
| `app/` | Rust workspace: core library, daemon (`omenctld`), CLI (`omenctl`) |

### Driver changes (`hpwmi-upstream/backport.patch`)

On top of mainline `hp-wmi` (which already supports 8D3F since v7.4-rc1):

1. builds on Ubuntu's 7.0 HWE kernel (`ACPI_AC_CLASS`);
2. `omenctl_concurrent_tdp` (write-only, accepts only `75`, 8D3F only);
3. kernel fan watchdog: `omenctl_kick`; if userspace stops kicking for 15 s while fans are
   manual, the driver forces both fans to the top table level;
4. smooth fan handover on 8D3F: returning fans to BIOS writes the current speed instead of 0.

## Safety design

- No direct EC writes; userspace only touches standard sysfs files.
- Fans go manual before any profile switch; BIOS-auto fans are refused in Unleashed.
- Four layers: kernel watchdog, systemd watchdog, rpm plausibility check, thermal
  protection that lowers power. Every failure path ends with more cooling.

## Build / test

```bash
cd app && cargo test -p omenctl-core
```

## License

GPL-2.0-or-later (the driver is derived from the Linux kernel's `hp-wmi.c`). See `LICENSE`.

## References

- Linux `drivers/platform/x86/hp/hp-wmi.c`
- [ohman](https://github.com/P4R1H/ohman) firmware research
- [OmenMon-Reborn](https://github.com/metamountain/OmenMon-Reborn-8BCA) (8D3F max-fan freeze report)
- [hp-omen-8603-linux](https://github.com/PickleHik3/hp-omen-8603-linux)
