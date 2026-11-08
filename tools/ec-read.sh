#!/bin/bash
# Read-only snapshot of the HP OMEN 8D3F EC fields we care about.
# Loads ec_sys WITHOUT write_support (read-only, file mode 0400), reads a few bytes, unloads.
# Offsets come from the decompiled DSDT (BIOS F.14), see acpi/dsdt.dsl Field (ERAM ...).
set -u
F=/sys/kernel/debug/ec/ec0/io
loaded_here=0
if [ ! -e "$F" ]; then
    modprobe ec_sys || exit 1
    loaded_here=1
fi
rd() { dd if="$F" bs=1 skip=$(( $1 )) count=1 status=none | od -An -tu1 | tr -d ' '; }
printf 'mode HPCM(0x95)=%s  heartbeat OMCC(0x62)=%s  timer(0x63)=%s  maxfan FFFS(0xEC)=%s\n' \
    "$(rd 0x95)" "$(( $(rd 0x62) & 1 ))" "$(rd 0x63)" "$(( ($(rd 0xEC) >> 2) & 1 ))"
printf 'set   PL1(0x37)=%s PL2(0x38)=%s PL4(0x39)=%s\n' "$(rd 0x37)" "$(rd 0x38)" "$(rd 0x39)"
printf 'EFFECTIVE PL1(0x3A)=%s W  PL2(0x3B)=%s W  PL4(0x3C)=%s W\n' "$(rd 0x3A)" "$(rd 0x3B)" "$(rd 0x3C)"
printf 'gpu dState NVDO(0x90)=%s ECDO(0x91)=%s\n' "$(rd 0x90)" "$(rd 0x91)"
[ "$loaded_here" = 1 ] && modprobe -r ec_sys
exit 0
