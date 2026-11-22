# Contributing to omenctl

Thanks for helping. omenctl talks to firmware that controls cooling, so the bar for
changes that reach the hardware is higher than usual. Reports and test results are as
valuable as code.

## Reporting bugs and test results

Use the issue templates. For anything hardware related, include:

- `cat /sys/class/dmi/id/board_name /sys/class/dmi/id/bios_version`
- `uname -r` and the omenctl version (`dpkg -s omenctl | grep Version`)
- `omenctl status` and `journalctl -u omenctld -b --no-pager | tail -50`
- `cat /sys/devices/platform/hp-wmi/omenctl_kick` if fans are involved
- `sudo tools/ec-read.sh` output if power limits are involved (read-only)

Do not attach ACPI tables (`acpi/`, `*.dat`, `*.dsl`). They are HP's code, and the MSDM
table contains your Windows product key. Quote the relevant method instead.

## Building and testing

```bash
sudo apt install pkg-config libwayland-dev libxkbcommon-dev dpkg-dev   # build dependencies
cd app
cargo test --locked           # unit tests + end-to-end test of omenctld on a fake sysfs tree
cargo clippy --workspace
cd .. && packaging/build-deb.sh
```

Rust 1.97 or newer is required (Ubuntu's `cargo-1.97` works:
`CARGO=cargo-1.97 packaging/build-deb.sh`). None of this needs the target laptop or
changes the system.

The patched driver lives in `hpwmi-upstream/src/hp-wmi.c`, and
`hpwmi-upstream/backport.patch` must stay equal to
`diff -u hp-wmi-upstream-e931e54.c src/hp-wmi.c` (see
[hpwmi-upstream/README.md](hpwmi-upstream/README.md)). Regenerate the patch whenever you
change the driver.

## What changes need

- **Logic in `omenctl-core`** — a unit test in the same file. The controller tests use a
  simulated machine (`Mock`) that records every write in order; safety-related changes
  should assert the order of writes, not only the end state.
- **Service, socket protocol, config** — keep `omenctld/tests/e2e.rs` passing and extend
  it when behaviour visible over the socket changes. New config keys need a safe default,
  validation, an entry in `packaging/config.toml` and in `docs/USER_GUIDE.md`.
- **Anything that sends a new firmware command or value** — evidence from the hardware:
  the BIOS method that handles it (from your own `tools/dump-acpi.sh` output), what
  OMEN Gaming Hub sends, and a test log with EC readings before and after. Only values
  observed from OMEN Gaming Hub are accepted. The firmware max-fan switch (`0x27`) stays
  unused.
- **Driver changes** — restricted to board `8D3F` with `dmi_match`, failing towards more
  cooling. Note the matching step in `docs/HARDWARE_TEST.md`.

## Code style

- Rust: match the surrounding code. The tree is not rustfmt-formatted, so do not reformat
  whole files in a functional change. No new `unsafe` without a `// SAFETY:` comment, no
  panics on the control path.
- Shell: POSIX `sh` for packaging scripts, `bash` only where already used; run
  `shellcheck` on the scripts you touch and add no new warnings.
- C: kernel style, as in the surrounding `hp-wmi.c`.
- Comments explain why (register meanings, firmware quirks, units, references), not what
  the next line does.

## Pull requests

- One topic per pull request, with a short description of what changed and how you tested
  it (unit tests only, or on hardware with which BIOS).
- Update `CHANGELOG.md` under "Unreleased" for user-visible changes.
- Contributions are accepted under the project's license (GPL-2.0-or-later).
