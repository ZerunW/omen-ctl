# Security policy

omenctl runs a root service (`omenctld`), installs a kernel module through DKMS and
changes how the embedded controller runs the fans and power limits. Bugs here can affect
the safety of the machine, so please report them privately.

## Supported versions

Only the latest release receives fixes.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting:
<https://github.com/ZerunW/omen-ctl/security/advisories/new>.
Please do not open a public issue for security problems.

Include the omenctl version, kernel version, board and BIOS version, and steps to
reproduce. You should get a reply within a week.

## In scope

- Privilege escalation through `/run/omenctl.sock` (it is writable by the `omenctl`
  group; requests from group members must not be able to do more than the documented
  commands).
- Ways to make the service write values outside the validated ranges (fan levels
  18-52, RAPL limits, curves, config keys) or to send firmware commands other than the
  documented ones.
- Ways to defeat the safety layers: the kernel fan watchdog, the systemd watchdog and
  `omenctl-safe-exit`, the fan speed check, thermal protection.
- The patched `hp-wmi` sysfs attributes (`omenctl_kick`, `omenctl_concurrent_tdp`,
  `omenctl_adapter_w`); the writable ones accept writes from root only.
- Maintainer scripts in `packaging/debian/` and the files they install.

## Hardening notes

- Members of the `omenctl` group can change performance and fan modes. Add only users you
  would let change cooling settings.
- The service unit runs with `ProtectSystem=strict`, `ProtectHome=yes` and
  `NoNewPrivileges=yes`, and writes only `/etc/omenctl`, `/var/lib/omenctl` and `/run`.
- The service and the safety script refuse to act on any board other than `8D3F`.
