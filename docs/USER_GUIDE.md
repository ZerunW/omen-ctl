# omenctl user guide

omenctl controls performance mode and fans on the **HP OMEN 16-am0xxx (board 8D3F)**
under Linux. It has four parts:

| Part | What it is | Runs as |
|---|---|---|
| `hp-wmi` (patched) | Kernel driver, built with DKMS. Sends the firmware commands. | kernel |
| `omenctld` | Background service. Applies the mode, drives the fans, watches temperatures. | root (systemd) |
| `omenctl` | Command line tool. | your user |
| `omenctl-gui` | Desktop app ("OMEN Control"). | your user |

The command line tool and the desktop app only talk to the service through the socket
`/run/omenctl.sock`. Only the service writes to the hardware, and it only uses standard
sysfs files. It never writes to the embedded controller (EC) directly.

## Contents

1. [Requirements](#1-requirements)
2. [Build the package](#2-build-the-package)
3. [Install](#3-install)
4. [Modes](#4-modes)
5. [Fans](#5-fans)
6. [Desktop app](#6-desktop-app)
7. [Command line](#7-command-line)
8. [Configuration](#8-configuration)
9. [Safety](#9-safety)
10. [Behaviour around sleep, power and other tools](#10-behaviour-around-sleep-power-and-other-tools)
11. [Troubleshooting](#11-troubleshooting)
12. [Uninstall and rollback](#12-uninstall-and-rollback)

## 1. Requirements

- HP OMEN 16-am0xxx, board `8D3F` (`cat /sys/class/dmi/id/board_name`). On any other board
  the service refuses to start and its safety script does nothing.
- Ubuntu 24.04 with a Linux **7.x** kernel (tested: HWE 7.0). The driver is only built for 7.x.
- Kernel headers for the running kernel (`linux-headers-generic-hwe-24.04`).
- Secure Boot: Ubuntu's DKMS signs the module with the machine owner key (MOK) in
  `/var/lib/shim-signed/mok/`. That key must be enrolled once (`mokutil --list-enrolled`).
- NVIDIA driver with `nvidia-powerd` (from Ubuntu's NVIDIA packages) for the GPU power boost.
- To build: Rust 1.97 or newer (`rustc`/`cargo`, or Ubuntu's `rustc-1.97`/`cargo-1.97`),
  `dpkg-dev` and internet access for the Rust crates.

## 2. Build the package

```bash
git clone https://github.com/ZerunW/omen-ctl
cd omen-ctl
packaging/build-deb.sh                      # or: CARGO=cargo-1.97 packaging/build-deb.sh
```

Result: `packaging/out/omenctl_<version>_amd64.deb`. Building changes nothing on the system.

To run the tests first:

```bash
cd app && cargo test
```

## 3. Install

```bash
sudo apt install ./packaging/out/omenctl_0.1.0_amd64.deb
sudo usermod -aG omenctl $USER              # then log out and in again
sudo reboot                                 # loads the patched driver
```

What the installation does (all of it is undone by `apt remove`, see
[section 12](#12-uninstall-and-rollback)):

| Step | Detail |
|---|---|
| Group `omenctl` | Members may use the socket. Created only if missing. |
| Driver | DKMS builds `hp-wmi` into `/lib/modules/<kernel>/updates/dkms/`, which is preferred over the stock module. Rebuilt for every new kernel. |
| Service | `omenctld.service` is **enabled for the next boot but not started**. |
| NVIDIA | Enables `nvidia-powerd` (or the bundled `omenctl-nvidia-powerd.service` if the driver has no unit) and `nvidia-suspend/resume/hibernate`. The previous state of these units is saved in `/var/lib/omenctl/rollback/units`. |
| Config | Copies `/usr/share/omenctl/config.toml` to `/etc/omenctl/config.toml` if it does not exist. |

After the reboot, check:

```bash
modinfo -n hp_wmi                            # .../updates/dkms/hp-wmi.ko*
cat /sys/devices/platform/hp-wmi/omenctl_kick
systemctl status omenctld
omenctl status
```

If the patched driver is not loaded, the service is skipped (not failed) and logs why.

## 4. Modes

| Mode | Platform profile | CPU limits (EC) | GPU | Fans |
|---|---|---|---|---|
| Power Saver | `low-power` | firmware default | default | your choice |
| Balanced | `balanced` | firmware default | default | your choice |
| Unleashed | `performance` | PL1 90 W / PL2 130 W / PL4 200 W | power limit 115 W | software only |

Unleashed only holds while the fans are software controlled: the firmware keeps the
90/130 W state only inside the window the driver refreshes in manual fan mode. That is
why **Auto** fans are not available in Unleashed.

Switching order (enforced by the service):

1. fans to manual at a safe level,
2. platform profile,
3. in Unleashed: CPU+GPU concurrent power target 75 W (this lifts the GPU limit to 115 W),
4. fans follow your fan setting.

The mode survives reboots (`/var/lib/omenctl/state.json`).

## 5. Fans

Fan speeds are set in levels of 100 rpm from **18 (1800 rpm) to 52 (5200 rpm)**.
The driver never writes 0 (on this EC, 0 means stop).

| Fan mode | Behaviour |
|---|---|
| Auto | The BIOS controls the fans. Not available in Unleashed. |
| Curve (default) | Each fan follows its curve, using the hotter of CPU and GPU. Speeds rise at once and fall gradually. |
| Manual | Both fans at one fixed speed. |
| Max | Both fans at 5200 rpm. The firmware's own "max fan" switch is never used (it can freeze the fan controller on this model). |

**Recommended speed.** The service always computes what the curves would ask for at the
current temperature. A manual speed below it is applied as you set it, but reported:
an amber line in the desktop app and an `alert` line in `omenctl status`.

**Fan curves.** Each fan (CPU, GPU) has its own curve of points (temperature, speed).
The default is the BIOS fan table. The temperatures assigned to its rows are provisional
until calibrated on the machine (see [HARDWARE_TEST.md](HARDWARE_TEST.md), "Later").
Curves must not go down as the temperature rises; the editor and the service both enforce it.

## 6. Desktop app

Start **OMEN Control** from the application menu, or run `omenctl-gui`.

- **Mode** and **Fan control** switch immediately.
- **Manual speed**: moving the slider switches to Manual when you let go. The tick on the
  slider marks the recommended speed; it turns amber when you are below it.
- The strip in the middle shows CPU and GPU temperature, power and fan speeds with the last
  60 seconds. "Idle" means the GPU is asleep; it is not woken up to be measured.
- **Fan curve** (collapsed by default): drag the points, then **Apply**. **Revert** drops
  your edits, **Reset** loads the BIOS table into the editor (applied only with Apply).
- Hover the (i) icons for explanations.

Options: `--theme light|dark|system` (default: follow the system), `--socket PATH`.

## 7. Command line

```bash
omenctl status            # or: omenctl status --json
omenctl mode saver        # saver | balanced | unleashed
omenctl fan curve         # auto | curve | max | 18..52
omenctl fan 30            # manual, 3000 rpm
```

Errors (for example `fan auto` in Unleashed) are printed and the exit code is 1.

## 8. Configuration

`/etc/omenctl/config.toml`, commented. Apply changes with
`sudo systemctl restart omenctld`. Every setting has a safe default; an invalid file makes
the service refuse to start (see `journalctl -u omenctld`).

| Setting | Default | Meaning |
|---|---|---|
| `default_mode` | `balanced` | Mode on the very first start |
| `unleashed_concurrent_75w` | `true` | Set the 75 W concurrent target in Unleashed |
| `[rapl] <mode> = [PL1, PL2]` | not set | Optional intel_rapl limits per mode (W, PL2 at most 130) |
| `[fan] fall` | `0.4` | How fast speeds decrease (share of the gap per second) |
| `[fan.cpu_curve]`, `[fan.gpu_curve]` | BIOS table | Written by the desktop app on Apply |
| `[protect] fans_max_at` | `92` | Both fans to maximum at this temperature (°C) |
| `[protect] drop_at`, `drop_after_s` | `95`, `10` | Leave Unleashed after this long at this temperature |
| `[protect] restore_below`, `restore_after_s` | `80`, `30` | Return to the selected mode after cooling |
| `[protect] rpm_ratio`, `rpm_fail_after_s` | `0.7`, `10` | Fan fault: measured below target × ratio for this long |

## 9. Safety

Every failure path ends with more cooling and less power.

1. **Kernel watchdog** (driver): while fans are manual, the service writes
   `/sys/devices/platform/hp-wmi/omenctl_kick` every second. Without a write for 15 s the
   driver sets both fans to the top level by itself.
2. **systemd watchdog**: the service reports to systemd (`WatchdogSec=15`); a hung
   service is killed and restarted. After any abnormal stop,
   `/usr/lib/omenctl/omenctl-safe-exit` sets fans to maximum and the profile to balanced,
   and gives the fans back to the BIOS only once the CPU is below 60 °C (the EC stops the
   fans for about two minutes when it takes over).
3. **Fan check**: measured speed below 70 % of the target for 10 s counts as a fault:
   fans to maximum, Unleashed is left, an alert is shown until you pick a mode again.
4. **Temperature**: at 92 °C fans go to maximum in every fan mode (including Auto); 10 s at
   95 °C leaves Unleashed until 30 s below 80 °C. A missing CPU temperature is treated as hot.

## 10. Behaviour around sleep, power and other tools

- **Sleep**: after resume the service re-applies the full mode switch (the EC can lose its
  state while asleep).
- **AC adapter**: in Unleashed, plugging or unplugging re-applies the 75 W target.
- **GNOME power menu / power-profiles-daemon**: if something else changes the platform
  profile, the service adopts it as the new mode (with the safe switching order).
- **GPU**: the GPU temperature is read through NVML only while the GPU is already awake,
  at most every 5 s, and NVML is unloaded after each read so the GPU can sleep.
- **Boot**: loading the driver briefly hands the fans to the EC (they may stop for about
  two minutes while the machine is idle); the service takes over when it starts.

## 11. Troubleshooting

| Symptom | Check |
|---|---|
| `omenctl: cannot reach the omenctl service` | `systemctl status omenctld`; `journalctl -u omenctld -b` |
| `no permission to use /run/omenctl.sock` | `id` must list `omenctl`; log out and in after `usermod` |
| Service "skipped" at boot | `modinfo -n hp_wmi` must point to `updates/dkms`; `dkms status`; `journalctl -k -b \| grep -i hp_wmi` |
| Driver does not load (Secure Boot) | `mokutil --sb-state`; key enrolled? `modinfo hp_wmi \| grep signer` |
| GPU stays at the old limit | `systemctl status nvidia-powerd` (or `omenctl-nvidia-powerd`); `nvidia-smi -q -d POWER` |
| Fans stuck at full speed | `cat /sys/devices/platform/hp-wmi/omenctl_kick` (`tripped=1` means the kernel watchdog fired: is the service running?) |

## 12. Uninstall and rollback

```bash
sudo apt remove omenctl        # keep config and state
sudo apt purge omenctl         # also delete /etc/omenctl, /var/lib/omenctl, the group (if created by omenctl)
sudo reboot                    # back to the stock hp-wmi driver
```

`remove` stops the service (balanced profile, fans back to the BIOS), removes the DKMS
driver, disables `omenctl-nvidia-powerd` and restores the recorded state of the NVIDIA units.

Emergency, without the package tools:

```bash
sudo systemctl disable --now omenctld
sudo dkms remove omenctl-hp-wmi/0.1.0 --all
sudo reboot
```

If the system does not boot normally, choose another kernel in the GRUB menu ("Advanced
options"): the driver is only built for 7.x kernels.
