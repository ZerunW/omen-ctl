# Changelog

All notable changes. Versions are the `.deb` / workspace version.

## 0.2.6 - 2026-11-08

- Upgrades no longer leave the service stopped: if it was running and the driver in
  memory is exactly the newly installed one, it is started again. If the driver
  changed, the installer says to reboot.

## 0.2.5 - 2026-11-08

- Fixed: `unleashed_concurrent_75w` in the example config sat below `[modes]`, so TOML
  read it as part of `[modes]` and setting it to `false` had no effect. It is now at the
  top. Configs from 0.2.0-0.2.4 that still have it under `[modes]` are honoured.
- The service now logs unknown settings in `/etc/omenctl/config.toml` (typos or a key
  under the wrong table) instead of silently using the default.
- A failed driver build during install now prints a clear warning with the cause and
  the command to retry.
- Licence notices of all Rust libraries are installed in
  `/usr/share/doc/omenctl/THIRD-PARTY-LICENSES.txt.gz`.
- The decompiled HP ACPI tables were removed from the repository; `tools/dump-acpi.sh`
  extracts them from your own machine.
- CI: build and test run with a read-only token; only the release job can write.

## 0.2.4 - 2026-11-08

- Desktop app: Auto / Light / Dark switch in the header; the choice is saved in
  `~/.config/omenctl/gui.toml` (`--theme` still overrides it for one run).

## 0.2.3 - 2026-11-08

- Fixed: the service could hang while the system prepared to sleep. NVML was called
  on the control loop while nvidia-suspend had already suspended the GPU; the call
  blocked until resume, systemd's watchdog killed the service and the fans were
  handed to the BIOS. GPU readings now run on their own thread; a stuck reading
  only makes the GPU temperature "unknown".
- Unleashed keeps the CPU energy/performance preference (EPP) at
  `balance_performance` (power-profiles-daemon sets `performance`). An idle
  desktop drops from about 45 W to about 25 W; the 90/130 W limits are unchanged.
  New option `[cpu] unleashed_epp`.

## 0.2.2 - 2026-11-08

- Fan curves use the hotter of CPU and GPU averaged over 5 s, hold a raised speed
  for 15 s, then slow down by 50 rpm per second. One-second load spikes no longer
  make the fans pump. Emergency full speed at 92 °C still uses the current reading.
  New options `[fan] average_s`, `hold_s`, `fall_per_s` (replace `fall`).
- New app icon.

## 0.2.1 - 2026-11-08

- Fixed: adapter rating is in units of 5 W (230 W adapter was shown as 460 W).
- Low battery rule: switch to a chosen mode once when the battery falls to a
  threshold (default 20 %) while unplugged.

## 0.2.0 - 2026-11-08

- Desktop app redesigned as a data panel: stat tiles, 90 s power chart (CPU, GPU,
  whole system on battery), fan card, power supply card, default modes card.
  The manual speed slider only shows in Manual mode.
- Default modes: at startup, when plugged in, on battery (`[modes]` in the config,
  `omenctl defaults ...`).
- Power supply readout: adapter rating, battery charge, draw, time left, health,
  cycles, USB-C power input. Driver: read-only `omenctl_adapter_w` (BIOS query 0x0F).

## 0.1.0 - 2026-11-08

First packaged version.

- Patched `hp-wmi` (DKMS): concurrent CPU+GPU power target, kernel fan watchdog,
  smooth fan handover to the BIOS.
- `omenctld` service: Power Saver / Balanced / Unleashed, fan curves, manual and
  maximum fans, thermal and fan-fault protection, systemd watchdog, safety script.
- `omenctl` command line tool and `omenctl-gui` desktop app.
- `.deb` with DKMS, nvidia-powerd setup and full rollback on removal.
