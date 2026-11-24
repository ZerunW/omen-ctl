# omenctl user guide

omenctl controls performance mode, fans and battery charging on the **HP OMEN 16-am0xxx (board 8D3F)**
under Linux. It has four parts:

| Part | What it is | Runs as |
|---|---|---|
| `hp-wmi` (patched) | Kernel driver, built with DKMS. Sends the firmware commands. | kernel |
| `omenctld` | Background service. Applies the mode, drives the fans, watches temperatures. | root (systemd) |
| `omenctl` | Command line tool. | your user |
| `omenctl-gui` | Desktop app ("OMEN Control"). | your user |
| `omenctl-web` | Optional read-only web dashboard, off by default. | unprivileged throwaway user (systemd) |

The command line tool and the desktop app only talk to the service through the socket
`/run/omenctl.sock`. Only the service writes to the hardware, and it only uses standard
sysfs files. It never writes to the embedded controller (EC) directly. How the driver
talks to the firmware is described in [internals.md](internals.md).

## Contents

1. [Requirements](#1-requirements)
2. [Get the package](#2-get-the-package)
3. [Install](#3-install)
4. [Modes](#4-modes)
5. [Fans](#5-fans)
6. [Charge limit](#6-charge-limit)
7. [Lid close](#7-lid-close)
8. [Remote monitor](#8-remote-monitor)
9. [Desktop app](#9-desktop-app)
10. [Command line](#10-command-line)
11. [Configuration](#11-configuration)
12. [Safety](#12-safety)
13. [Behaviour around sleep, power and other tools](#13-behaviour-around-sleep-power-and-other-tools)
14. [Troubleshooting](#14-troubleshooting)
15. [Uninstall and rollback](#15-uninstall-and-rollback)

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

## 2. Get the package

Easiest: download `omenctl_<version>_amd64.deb` from the
[latest release](https://github.com/ZerunW/omen-ctl/releases/latest) and skip to
[section 3](#3-install). To build it yourself:

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
sudo apt install ./omenctl_0.3.0_amd64.deb  # or ./packaging/out/... if you built it
sudo usermod -aG omenctl $USER              # then log out and in again
sudo reboot                                 # loads the patched driver
```

What the installation does (`apt remove` undoes it and keeps your config; `apt purge`
also deletes config, state and the group; see
[section 15](#15-uninstall-and-rollback)):

| Step | Detail |
|---|---|
| Group `omenctl` | Members may use the socket (modes, fan control). Created only if missing. |
| Driver | DKMS builds `hp-wmi` into `/lib/modules/<kernel>/updates/dkms/`, which is preferred over the stock module. Rebuilt for every new kernel. |
| Service | `omenctld.service` is **enabled for the next boot but not started** on a first install (the stock driver is still loaded). On an upgrade it is started again right away when it was running and the driver did not change; if the driver changed, the installer asks for a reboot. |
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

In Unleashed the CPU energy preference (EPP) is switched automatically: `performance`
while any CPU is busy (at least 50 % of a second), `balance_performance` after 10 s
without such load. Measured on 8D3F:

| EPP | single core | all cores (130 W / 90 W) | idle CPU power |
|---|---|---|---|
| `balance_performance` | 3.8 GHz | 3.8 GHz, does not reach the power limit | about 16 W |
| `performance` | 4.85 GHz | P 4.6 / E 3.7 GHz, then P 4.2 / E 3.4 GHz | about 31 W |

Set `[cpu] unleashed_epp` to a fixed value to turn the switching off.

CPU and GPU share one power budget of about 175 W in Unleashed; NVIDIA Dynamic Boost
gives the GPU what the CPU leaves. With both at full load the CPU takes its 90 W and the
GPU is held at 85 W. **GPU priority** lowers the CPU sustained limit (RAPL PL1) to 70 W
while the GPU draws 50 W or more (for 3 s); measured on 8D3F the GPU limit then rises to
105 W. The previous limit returns after the GPU stays below 30 W for 15 s, when leaving
Unleashed and when the service stops. `omenctl status` shows "GPU priority" while it is
on. Set `[cpu] gpu_priority_pl1` to another value (35-90) or 0 to turn it off.

Unleashed only holds while the fans are software controlled: the firmware keeps the
90/130 W state only inside the window the driver refreshes in manual fan mode. That is
why **Auto** fans are not available in Unleashed.

Switching order (enforced by the service):

1. fans to manual at a safe level,
2. platform profile,
3. in Unleashed: CPU+GPU concurrent power target 75 W (this lifts the GPU limit to 115 W),
4. fans follow your fan setting.

The mode survives reboots (`/var/lib/omenctl/state.json`).

**Default modes.** Four optional rules, set in the desktop app ("Default modes"), with
`omenctl defaults`, or in the `[modes]` table of the config:

| Rule | Values | Meaning |
|---|---|---|
| At startup | Last used, or a mode | Mode when the service starts (boot). With "Last used", a matching plugged in / on battery rule wins; otherwise the mode from before the reboot is kept. |
| Plugged in | No change, or a mode | Switch to this mode when the adapter is connected. |
| On battery | No change, or a mode | Switch to this mode when the adapter is removed. |
| Low battery | No change, or a mode | Switch to this mode once when the charge falls to the threshold (default 20%, 5 to 50) while unplugged. It acts again after plugging in or after the charge rose 5% above the threshold. It also wins at startup when the battery is already that low. |

The power-source rules act only when the power source changes (and at startup, as above).
You can still change the mode by hand at any time. If overheat protection is holding the
machine in Balanced when you plug in, Unleashed is resumed once it has cooled down.

## 5. Fans

Fan speeds are set in levels of 100 rpm from **18 (1800 rpm) to 52 (5200 rpm)**.
The driver never writes 0 (on this EC, 0 means stop).

| Fan mode | Behaviour |
|---|---|
| Auto | The BIOS controls the fans. Not available in Unleashed. |
| Curve (default) | Each fan follows its curve, using the hotter of CPU and GPU averaged over 5 seconds, so short load spikes don't move the fans. Speeds rise at once, are held for 15 seconds, then fall by 50 rpm per second. At 92 °C both fans go to full speed at once (no averaging). |
| Manual | Both fans at one fixed speed. |
| Max | Both fans at 5200 rpm. The firmware's own "max fan" switch is never used (it can freeze the fan controller on this model). |

**Recommended speed.** The service always computes what the curves would ask for at the
current temperature. A manual speed below it is applied as you set it, but reported:
an amber line in the desktop app and an `alert` line in `omenctl status`.

**Fan curves.** Each fan (CPU, GPU) has its own curve of points (temperature, speed).
The default is the BIOS fan table. The temperatures assigned to its rows are provisional
until calibrated on the machine (see [HARDWARE_TEST.md](HARDWARE_TEST.md), "Later").
Curves must not go down as the temperature rises; the editor and the service both enforce it.

## 6. Charge limit

For a laptop that stays plugged in: charging stops at a set level and starts again once
the battery has dropped a few percent, so the battery does not sit at 100 %. Off by default.

```bash
omenctl charge 60 55      # stop at 60 %, charge again at 55 %
omenctl charge 80         # stop at 80 %, charge again at 75 %
omenctl charge            # show the state
omenctl charge full       # charge to 100 % once (for a trip); ends when unplugged or full
omenctl charge off        # charge to 100 % as usual
```

The desktop app has the same in the Power supply card. The setting is saved in
`[battery]` of `/etc/omenctl/config.toml`.

How it works: the service writes the kernel's standard battery property
`/sys/class/power_supply/BAT0/charge_behaviour`: `inhibit-charge` at the limit (the laptop
then runs from the adapter and the battery stays where it is), `auto` at the resume level.
The patched driver provides that property on 8D3F (the mainline driver offers it only on a
few other HP models); the firmware commands behind it are listed in
[internals.md](internals.md).

- On battery the limit does nothing; charging is set back to `auto` as soon as the adapter
  is unplugged.
- Powering off or rebooting keeps the limit: the service leaves `inhibit-charge` set, and
  the firmware keeps it while the laptop is off and plugged in.
- Stopping the service (`systemctl stop`), a crash (via the safety script) and
  uninstalling set `auto` again: the worst case is a battery that charges to 100 %.
- Holding means the battery is neither charged nor used, so a battery that is already
  above the limit stays there while plugged in. Run on battery once to bring it down.
- Lithium-ion batteries age fastest when kept full and warm. Laptop makers' "always
  plugged in" modes stop at 55-60 % (Lenovo conservation mode, ASUS maximum lifespan) or
  80 %.

## 7. Lid close

Keep the laptop running with the lid closed, for example to use it as a small home server.

```bash
omenctl lid ignore-on-ac  # keep running while plugged in, suspend on battery
omenctl lid ignore        # always keep running
omenctl lid suspend       # desktop default
omenctl lid               # show the setting
```

Also in the desktop app (Power supply card), saved in `[lid]` of the configuration.

The service holds a systemd-logind inhibitor lock (`handle-lid-switch`) while the lid
should be ignored; `systemd-inhibit --list` shows it as `omenctl`. Nothing is written to
the logind configuration, and the lock disappears when the service stops, so closing the
lid suspends again. Notes:

- With the lid closed, the desktop turns off the built-in screen when external monitors
  are connected. Without external monitors the built-in screen may stay on behind the lid
  until the desktop blanks it after its idle time (Settings > Power > Screen Blank).
- Heat leaves through the keyboard deck less well with the lid closed; watch the
  temperatures under long heavy load.
- GNOME's own idle suspend is separate (Settings > Power > Automatic Suspend). For a
  server, turn it off for "Plugged In".

## 8. Remote monitor

`omenctl-web` is a read-only dashboard for a browser on another device: usage of every
CPU core, memory, Intel and NVIDIA GPU usage, the processes `nvidia-smi` lists, every
process (sortable, searchable), fans, temperatures, power and battery, charge limit,
network rates and VPN state (GlobalProtect, Tailscale). Nothing can be changed through it.

It is installed but not enabled. It listens on `127.0.0.1:8787` (this machine only), so it
is meant to be reached through a tunnel or reverse proxy on the same machine:

```bash
sudo systemctl enable --now omenctl-web
curl -s http://127.0.0.1:8787/healthz        # "ok"
```

It runs as an unprivileged throwaway user (systemd `DynamicUser`) and reads the service
status through the read-only socket `/run/omenctl-status.sock` (which accepts nothing but
status requests), so even a compromised web process cannot change any setting. It samples
only while someone is watching (a request in the last
15 s), so an unwatched dashboard costs nothing and never wakes the NVIDIA GPU. While a page
is open it uses about 3 % of one CPU core.

Settings go into `/etc/omenctl/web.env` (then `sudo systemctl restart omenctl-web`):

| Variable | Default | Meaning |
|---|---|---|
| `OMENCTL_WEB_LISTEN` | `127.0.0.1:8787` | Address and port |
| `OMENCTL_WEB_TOKEN_FILE` | not set | File with a token: 16 or more characters from `A-Z a-z 0-9 - _` (e.g. `openssl rand -hex 24`), readable by the service (`chmod 644`). Then every request needs it: open `/?token=<token>` once per browser (stored in a cookie) or send `Authorization: Bearer <token>` |

The process list shows full command lines, which can contain paths and sometimes secrets
passed as arguments. Keep it behind a login.

**Cloudflare Tunnel with Cloudflare Access** (no port opened on the laptop or router;
needs a domain on Cloudflare). Set up the login before the hostname goes live:

1. Zero Trust > Access > Applications: add a self-hosted application for the hostname you
   will use (e.g. `omen.example.com`) with a policy that allows only your e-mail address
   (one-time PIN or another login). Note its Application Audience (AUD) tag.
2. Install `cloudflared` from Cloudflare's apt repository
   ([instructions](https://pkg.cloudflare.com/index.html)).
3. Zero Trust > Networks > Tunnels: create a tunnel and run the shown
   `sudo cloudflared service install <token>` on the laptop. Add the public hostname with
   service `http://127.0.0.1:8787`, and under its Access settings turn on
   "Protect with Access" (team name and the AUD tag from step 1), so `cloudflared` itself
   rejects requests without a valid Access token.
4. Open the hostname: Cloudflare asks for the login first.

**LAN or Tailscale** instead: set `OMENCTL_WEB_LISTEN=0.0.0.0:8787` and a token file, and
allow the port only from the networks you need (`sudo ufw allow from 192.168.1.0/24 to any
port 8787 proto tcp`). This is plain HTTP: the token and the data cross the LAN
unencrypted. Inside a Tailscale or WireGuard tunnel to the laptop that is fine; across a
shared LAN put an HTTPS reverse proxy in front.

## 9. Desktop app

Start **OMEN Control** from the application menu, or run `omenctl-gui`.

- **Mode** (top right) and the **Fans** buttons switch immediately.
- **Tiles**: CPU and GPU temperature and power, fan speeds, and the power source
  (adapter rating on AC; charge, draw and time left on battery). "Idle" means the GPU is
  asleep; it is not woken up to be measured.
- **Power chart**: CPU and GPU power over the last 90 seconds. On battery it also shows
  the whole machine's draw ("System"); Linux cannot measure that while on AC.
- **Fans**: the speed slider appears only in Manual. Selecting Manual starts from the
  current fan speed. The tick marks the recommended speed; a note appears when you are
  below it.
- **Fan curve** (open it from the Fans card): drag the points, then **Apply**. **Revert**
  drops your edits, **Reset to BIOS table** loads the BIOS table into the editor (applied
  only with Apply).
- **Power supply**: source, adapter rating, USB-C power input, battery state, time left,
  health (full capacity compared with the design capacity) and charge cycles.
- **Default modes**: the four rules and the low battery threshold described in [Modes](#4-modes); saved immediately.
- **Auto / Light / Dark** (top right): follow the desktop theme or force light or dark. The
  choice is saved in `~/.config/omenctl/gui.toml`.
- Hover the (i) icons for explanations.
- **Window**: the app draws its own frame (rounded corners, window buttons in the order set
  by the desktop's `button-layout`). Drag the top bar to move, double-click it to maximize,
  drag the edges to resize. Text follows the desktop text scaling (GNOME "Large text",
  `text-scaling-factor`) and grows with a larger window.

Options: `--theme light|dark|system` (for this run only; otherwise the saved choice), `--socket PATH`.

## 10. Command line

```bash
omenctl status            # or: omenctl status --json
omenctl mode saver        # saver | balanced | unleashed
omenctl fan curve         # auto | curve | max | 18..52
omenctl fan 30            # manual, 3000 rpm
omenctl defaults          # show the startup / plugged in / on battery rules
omenctl defaults ac unleashed      # startup | ac | battery, then keep | saver | balanced | unleashed
omenctl defaults startup keep      # keep = last used (startup) or no change (ac, battery, low)
omenctl defaults low saver         # mode when the battery runs low
omenctl defaults low-at 15         # low battery threshold, 5-50 %
omenctl charge 60 55      # charge limit (see section 6); off | full | full off
omenctl lid ignore-on-ac  # lid close (see section 7): suspend | ignore-on-ac | ignore
```

`omenctl status` also shows the power source, the adapter rating (when the driver reports
it), the battery and USB-C power input.

Errors (for example `fan auto` in Unleashed) are printed and the exit code is 1.

## 11. Configuration

`/etc/omenctl/config.toml`, commented. Apply changes with
`sudo systemctl restart omenctld`. Every setting has a safe default; an invalid file makes
the service refuse to start (see `journalctl -u omenctld`).

| Setting | Default | Meaning |
|---|---|---|
| `default_mode` | `balanced` | Mode on the very first start |
| `[modes] startup`, `on_ac`, `on_battery`, `on_low_battery` | `keep` | Default-mode rules (see [Modes](#4-modes)); written by the desktop app |
| `[modes] low_battery_percent` | `20` | Low battery threshold (5 to 50 %) |
| `unleashed_concurrent_75w` | `true` | Set the 75 W concurrent target in Unleashed |
| `[rapl] <mode> = [PL1, PL2]` | not set | Optional intel_rapl limits per mode (W, PL2 at most 130) |
| `[fan] average_s`, `hold_s`, `fall_per_s` | `5`, `15`, `0.5` | Curve temperature averaging (s), hold after a rise (s), fall rate (levels of 100 rpm per second) |
| `[fan.cpu_curve]`, `[fan.gpu_curve]` | BIOS table | Written by the desktop app on Apply |
| `[cpu] unleashed_epp` | `auto` | CPU EPP while in Unleashed (`auto` = by load, `keep` = leave it to power-profiles-daemon) |
| `[cpu] gpu_priority_pl1` | `70` | Unleashed: CPU sustained limit (W) while the GPU is busy, 0 = off |
| `[cpu] auto_busy_percent`, `auto_idle_s` | `50`, `10` | `auto`: a CPU busy at least this share of a second switches to `performance`; back after this many idle seconds |
| `[battery] charge_limit`, `charge_resume` | `0`, `55` | Charge limit (see [Charge limit](#6-charge-limit)); 0 = off, else 50 to 100 % |
| `[lid] close` | `suspend` | `suspend`, `ignore_on_ac` or `ignore` (see [Lid close](#7-lid-close)) |
| `[protect] fans_max_at` | `92` | Both fans to maximum at this temperature (°C) |
| `[protect] drop_at`, `drop_after_s` | `95`, `10` | Leave Unleashed after this long at this temperature |
| `[protect] restore_below`, `restore_after_s` | `80`, `30` | Return to the selected mode after cooling |
| `[protect] rpm_ratio`, `rpm_fail_after_s` | `0.7`, `10` | Fan fault: measured below target × ratio for this long |

## 12. Safety

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

## 13. Behaviour around sleep, power and other tools

- **Sleep**: after resume the service re-applies the full mode switch (the EC can lose its
  state while asleep).
- **AC adapter**: plugging or unplugging applies your plugged in / on battery rule; without
  a rule, Unleashed re-applies the 75 W target. A change while asleep is handled right
  after resume.
- **GNOME power menu / power-profiles-daemon**: if something else changes the platform
  profile, the service adopts it as the new mode (with the safe switching order).
- **GPU**: the GPU temperature is read through NVML only while the GPU is already awake,
  at most every 5 s, and NVML is shut down after each read so the GPU can sleep.
- **Boot**: loading the driver briefly hands the fans to the EC (they may stop for about
  two minutes while the machine is idle); the service takes over when it starts.

## 14. Troubleshooting

| Symptom | Check |
|---|---|
| `omenctl: cannot reach the omenctl service` | `systemctl status omenctld`; `journalctl -u omenctld -b` |
| `no permission to use /run/omenctl.sock` | `id` must list `omenctl`; log out and in after `usermod` |
| Service "skipped" at boot | `modinfo -n hp_wmi` must point to `updates/dkms`; `dkms status`; `journalctl -k -b \| grep -i hp_wmi` |
| Driver does not load (Secure Boot) | `mokutil --sb-state`; key enrolled? `modinfo hp_wmi \| grep signer` |
| GPU stays at the old limit | `systemctl status nvidia-powerd` (or `omenctl-nvidia-powerd`); `nvidia-smi -q -d POWER` |
| Fans stuck at full speed | `cat /sys/devices/platform/hp-wmi/omenctl_kick` (`tripped=1` means the kernel watchdog fired: is the service running?) |
| Bluetooth keyboard cannot wake the laptop from suspend | Not an OMEN problem (BlueZ); see [ubuntu-ble-wake-fix](https://github.com/ZerunW/ubuntu-ble-wake-fix) |

## 15. Uninstall and rollback

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
sudo dkms remove omenctl-hp-wmi/0.3.0 --all
sudo reboot
```

If the system does not boot normally, choose another kernel in the GRUB menu ("Advanced
options"): the driver is only built for 7.x kernels.
