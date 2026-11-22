# hpwmi-upstream

The Linux `hp-wmi` platform driver (`drivers/platform/x86/hp/hp-wmi.c`) and the omenctl
patch on top of it. The package builds `src/` with DKMS.

| File | What it is |
|---|---|
| `hp-wmi-upstream-e931e54.c` | Unmodified `hp-wmi.c` from mainline Linux at commit [`e931e54edb0f`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=e931e54edb0fae350007c4054cfbf095cc316bd8) ("platform/x86: hp-wmi: Disable hwmon attributes when no fans are usable"). This version already includes board `8D3F` support. |
| `hp-wmi-v7.0.c` | Unmodified `hp-wmi.c` as of Linux v7.0, the base of Ubuntu's 24.04 HWE kernel. Kept for comparison only; it has no 8D3F support and is not built. |
| `backport.patch` | `diff -u hp-wmi-upstream-e931e54.c src/hp-wmi.c`. |
| `src/hp-wmi.c` | The upstream file with `backport.patch` applied. This is what DKMS builds. |
| `src/Makefile` | Kbuild makefile (`obj-m += hp-wmi.o`). |

What the patch changes is described in [docs/internals.md](../docs/internals.md#5-driver-patch).
In short: it builds on the 7.0 kernel, and on 8D3F only it adds a write-only concurrent
power target, a kernel fan watchdog, a smooth fan handover to the BIOS and a read-only
adapter rating. All omenctl additions are marked with `omenctl` in comments and
identifiers.

## License and authorship

`hp-wmi.c` is part of the Linux kernel and is licensed under
**GPL-2.0-or-later** (see the SPDX line at the top of each file). Copyright belongs to its
authors, including Matthew Garrett, Red Hat, Anssi Hannula and the other contributors
listed in the file header and in the kernel's git history. The omenctl changes in
`backport.patch` are released under the same license.

Upstream source:
<https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/platform/x86/hp/hp-wmi.c>

## Rebuilding the patched file

```bash
cd hpwmi-upstream
cp hp-wmi-upstream-e931e54.c src/hp-wmi.c
patch -p0 src/hp-wmi.c < backport.patch
```

To build the module by hand against the running kernel (this does not install it):

```bash
make -C /lib/modules/$(uname -r)/build M=$PWD/src modules
```
