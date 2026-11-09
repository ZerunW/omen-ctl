# omenctl hardware test checklist

Step-by-step tests on the real machine (HP OMEN 16-am0xxx, board 8D3F, Ubuntu 24.04,
kernel 7.0). Do them in order; each step assumes the previous ones passed.
Every step lists its **rollback**. Stop at the first unexpected result and keep the output.

Keep a second terminal open with:

```bash
journalctl -f -u omenctld -k | grep -iE 'omenctl|hp_wmi|hp-wmi'
```

Useful read-only commands used below:

```bash
H=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
watch -n1 "cat $H/fan1_input $H/fan2_input $H/pwm1_enable /sys/firmware/acpi/platform_profile /sys/devices/platform/hp-wmi/omenctl_kick"
sudo tools/ec-read.sh          # EC snapshot (read-only): mode byte, effective PL1/PL2/PL4
```

**Emergency rollback at any point**

```bash
sudo systemctl stop omenctld                # safe stop: balanced, fans back to BIOS
H=$(ls -d /sys/devices/platform/hp-wmi/hwmon/hwmon*)
echo 2 | sudo tee $H/pwm1_enable            # fans back to BIOS by hand
echo balanced | sudo tee /sys/firmware/acpi/platform_profile
sudo apt remove omenctl && sudo reboot      # everything off, stock driver
```

---

## 1. Build the package

Purpose: the code builds on the machine's Rust 1.97 and all tests pass.

```bash
cd ~/omen-ctl/app && cargo test          # or cargo-1.97
cd .. && packaging/build-deb.sh
dpkg-deb -c packaging/out/omenctl_0.2.9_amd64.deb | less
```

Expected: all tests `ok`; the .deb lists `usr/sbin/omenctld`, `usr/bin/omenctl`,
`usr/bin/omenctl-gui`, `usr/src/omenctl-hp-wmi-0.2.9/`, the systemd units and docs.

Rollback: none needed (no system change).

## 2. Install the package

Purpose: maintainer scripts run cleanly; DKMS builds and signs the driver.

```bash
sudo apt install ./packaging/out/omenctl_0.2.9_amd64.deb
dkms status
modinfo /lib/modules/$(uname -r)/updates/dkms/hp-wmi.ko* | grep -E 'signer|srcversion|vermagic'
systemctl is-enabled omenctld nvidia-powerd omenctl-nvidia-powerd nvidia-suspend nvidia-resume nvidia-hibernate
cat /var/lib/omenctl/rollback/units
systemctl is-active omenctld
```

Expected: `omenctl-hp-wmi/0.2.9 ... installed`; signer is the machine's MOK key;
`omenctld` enabled but **inactive**; one of the two powerd units enabled; the rollback file
lists the units' state before installation.

Rollback: `sudo apt remove omenctl` (then check that `dkms status` no longer lists it and the
units are back to the recorded state).

## 3. Load the patched driver

Purpose: the DKMS module replaces the stock one without a reboot.

```bash
sudo rmmod hp_wmi && sudo modprobe hp_wmi
modinfo -n hp_wmi                             # .../updates/dkms/hp-wmi.ko*
cat /sys/devices/platform/hp-wmi/omenctl_kick # armed=0 tripped=0 ...
```

Expected: path under `updates/dkms`; `omenctl_kick` exists. The fans may stop for about
two minutes after loading (EC takes over), so do this while idle.

Rollback: `sudo dkms remove omenctl-hp-wmi/0.2.9 --all && sudo rmmod hp_wmi && sudo modprobe hp_wmi`
(loads the stock module again).

## 4. Start the service in Balanced

Purpose: first start, socket, systemd watchdog, kernel watchdog armed.

```bash
sudo usermod -aG omenctl $USER    # log out and in, then:
id | grep omenctl
sudo systemctl start omenctld
systemctl status omenctld          # active (running), "ready; socket /run/omenctl.sock"
ls -l /run/omenctl.sock            # srw-rw---- root omenctl
omenctl status
cat /sys/devices/platform/hp-wmi/omenctl_kick   # armed=1, since_kick 0 or 1
```

Expected: mode Balanced, fan mode curve, CPU temperature and both fan speeds shown,
fan targets follow the curve, no alerts.

Rollback: `sudo systemctl stop omenctld` (expect `pwm1_enable` back to 2).

## 5. Fan modes from the command line

Purpose: every fan mode works and the speeds follow.

| Command | Expected after ~10 s |
|---|---|
| `omenctl fan 30` | both fans about 3000 rpm |
| `omenctl fan 20` | about 2000 rpm; `alert ... below the recommended ...` if the curve wants more |
| `omenctl fan max` | about 5200 rpm |
| `omenctl fan curve` | back to the curve values |
| `omenctl fan auto` | `pwm1_enable` = 2, BIOS controls the fans |
| `omenctl fan 60` | refused, exit code 1 |

Rollback: `omenctl fan curve`.

## 6. Desktop app

Purpose: the app connects as a normal user and every control works.

```bash
omenctl-gui
```

Check: mode and fan control switch; manual slider applies on release and shows the amber
line below the recommended speed; the readout strip updates every second; GPU shows "Idle"
while the GPU sleeps; Fan curve opens, a point drags, **Apply** writes
`[fan.cpu_curve]` to `/etc/omenctl/config.toml`; hover on (i) shows the explanations;
light and dark system themes both look right.

Rollback: `Reset` then `Apply` in the app (or delete the `[fan.*_curve]` tables from the
config and restart the service).

## 7. Unleashed, CPU only

Purpose: EC switches to 90/130/200 W and stays there under sustained load; fans keep up.

```bash
omenctl mode unleashed
cat /sys/firmware/acpi/platform_profile       # performance
sudo tools/ec-read.sh                          # mode 0x31 (49), EFFECTIVE PL1 90 / PL2 130 / PL4 200
sudo turbostat --quiet --show PkgWatt,Busy%,Bzy_MHz,PkgTmp -i 2 &
stress-ng --cpu 24 --timeout 300s
```

Expected: package power holds about 90 W after the boost; temperature below 92 °C or fans
at 5200 rpm; `ec-read.sh` still shows 90/130/200 after 5 minutes; no alerts.

Rollback: `omenctl mode balanced`.

## 8. GPU limit in Unleashed

Purpose: concurrent target 75 W and nvidia-powerd lift the GPU limit to 115 W.

```bash
systemctl status nvidia-powerd || systemctl status omenctl-nvidia-powerd
omenctl mode unleashed
# start a GPU load (game or benchmark), then:
nvidia-smi -q -d POWER | grep -iE 'Current Power Limit|Power Draw'
```

Expected: current power limit 115 W (stock: about 80 W).

Rollback: `omenctl mode balanced`.

## 9. Kernel fan watchdog through the service

Purpose: if the service freezes, the driver forces the fans up by itself, and systemd
replaces the frozen service.

```bash
omenctl fan 25
sudo kill -STOP $(pidof omenctld)
# watch omenctl_kick and the fan speeds
```

Expected timeline:

| After | What happens |
|---|---|
| 15-20 s | kernel log `hp_wmi: omenctl watchdog: no kick for 15s, fans forced to max level`; `omenctl_kick` shows `tripped=1` (the driver checks every 5 s) |
| ~10 s later | both fans at about 5200 rpm (same ramp as `logs/wd-test-142901.log`) |
| 15 s after the last report | systemd's watchdog fires and sends SIGABRT, which a stopped process only receives once continued; systemd sends SIGKILL after `TimeoutStopSec` (150 s) at the latest |
| after the kill | `omenctl-safe-exit` runs (log line `daemon stopped abnormally`), then systemd restarts the service and `tripped` returns to 0 |

To finish the test without waiting: `sudo kill -CONT $(pidof omenctld)` (the pending SIGABRT
then ends the process and the restart follows).

```bash
journalctl -u omenctld -b --no-pager | tail -20
systemctl status omenctld          # new PID
```

Rollback: `sudo kill -CONT $(pidof omenctld)` if it is still stopped; `sudo systemctl restart omenctld`.

## 10. Safety script after a crash

Purpose: `omenctl-safe-exit` runs after an abnormal exit.

```bash
omenctl mode unleashed
sudo kill -KILL $(pidof omenctld)
journalctl -u omenctld -b --no-pager | tail -10
```

Expected: log line `omenctl-safe-exit: daemon stopped abnormally`; profile `balanced`; fans
at maximum until the CPU is below 60 °C (the script waits up to about 2 minutes), then back
to BIOS; only then does systemd restart the service (`RestartSec=5`), which applies the saved
mode (Unleashed) again.

Rollback: `sudo systemctl restart omenctld`.

## 11. Clean stop

Purpose: a normal stop leaves the machine in a safe state and the safety script does nothing.

```bash
sudo systemctl stop omenctld
cat /sys/firmware/acpi/platform_profile $H/pwm1_enable    # balanced, 2
journalctl -u omenctld -b --no-pager | tail -5            # "stopping: ...", no safe-exit line
```

Rollback: `sudo systemctl start omenctld`.

## 12. Profile changed by GNOME

Purpose: a change from the GNOME power menu is adopted with the safe switching order.

Pick "Performance" in the GNOME power menu, then `omenctl status`.

Expected: mode Unleashed, fans manual (curve), alert `platform profile was changed externally`
for one tick; picking "Balanced" in the menu returns to Balanced.

Rollback: `omenctl mode balanced`.

## 13. AC adapter in Unleashed

Purpose: the 75 W target is re-applied after unplugging and plugging the charger.

In Unleashed with a GPU load: unplug, wait 10 s, plug in, then run the command from step 8.

Expected: the limit returns to 115 W on AC.

Rollback: none needed.

## 14. Sleep and resume

Purpose: the mode is re-applied after resume.

```bash
omenctl mode unleashed
systemctl suspend          # wake it after ~30 s
journalctl -u omenctld -b --no-pager | grep -i resume
sudo tools/ec-read.sh
```

Expected: `resume from sleep detected: re-applying mode`; EC mode 0x31 and 90/130/200 W again;
fans manual.

Rollback: `omenctl mode balanced`.

## 15. The service does not keep the GPU awake

Purpose: polling never prevents the GPU from runtime suspend.

Close all GPU programs, keep the service and the desktop app running, wait 60 s:

```bash
cat /sys/bus/pci/devices/0000:01:00.0/power/runtime_status
sudo fuser -v /dev/nvidia* 2>&1 | grep omenctl || echo "omenctld holds no NVIDIA device"
```

Expected: `suspended`; omenctld does not hold `/dev/nvidia*`.

Rollback: if it stays `active` because of omenctld, report it; workaround:
add `--no-gpu` to `ExecStart` with `sudo systemctl edit omenctld`.

## 16. Reboot

Purpose: the service starts at boot with the saved mode.

```bash
sudo reboot
systemctl status omenctld
omenctl status
```

Expected: running, same mode and fan mode as before the reboot.

Rollback: `sudo systemctl disable omenctld`.

## 17. Uninstall

Purpose: `apt remove` restores the original system.

```bash
sudo apt remove omenctl
dkms status                         # no omenctl-hp-wmi
systemctl is-enabled omenctld 2>&1  # not found / disabled
cat /var/lib/omenctl/rollback/units # compare with the units' state now:
systemctl is-enabled nvidia-powerd nvidia-suspend nvidia-resume nvidia-hibernate
sudo reboot && modinfo -n hp_wmi    # stock path (kernel/drivers/platform/x86/hp/)
```

Expected: no service, no DKMS module, NVIDIA units as recorded, stock driver after reboot.
Then reinstall for daily use.

---

## Later (separate confirmation for each)

These need their own plan before running:

1. **Calibrate the BIOS fan table temperatures.** In Balanced with Auto fans, log
   temperature and fan speed (`omenctl status --json` every second) at idle, light and
   heavy load; assign a temperature to each of the 12 table rows and update the default
   curves.
2. **CPU and GPU full load together** in Unleashed (thermal headroom with both at maximum).
   Note for the test script: a bare `wait` also waits for `tee` started by
   `exec > >(tee ...)`; wait only on the load PIDs (`wait $CPU_PID $GPU_PID`).
3. **Mode byte 0x04** (recognised by the BIOS, not tested on Linux).
4. **GPU MUX switching** (iGPU / dGPU / Advanced Optimus), last, with a recovery path that
   works without a desktop.
