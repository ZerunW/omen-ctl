#!/bin/sh
# Extract and decompile the ACPI tables of THIS machine into ./acpi/ so you can
# follow the references in README.md (e.g. WMID.GSAS, GM2E, NPCF) on your own
# firmware. The tables are HP's code: they are not part of this repository and
# should not be published.
#
# Needs: sudo apt install acpica-tools   (provides acpidump, acpixtract, iasl)
# Reads only; writes nothing to the firmware.
#
# The MSDM table holds the Windows product key: it is deleted right away and
# never decompiled.
set -eu

out="${1:-acpi}"
mkdir -p "$out"
cd "$out"

sudo acpidump -o tables.dat
sudo chown "$(id -u):$(id -g)" tables.dat
acpixtract -a tables.dat >/dev/null
rm -f tables.dat msdm.dat
iasl -e ssdt*.dat -d dsdt.dat >/dev/null 2>&1 || iasl -d dsdt.dat >/dev/null
for f in ssdt*.dat; do iasl -d "$f" >/dev/null 2>&1 || true; done

echo "Decompiled tables in $(pwd):"
ls ./*.dsl
echo "BIOS: $(cat /sys/class/dmi/id/bios_version 2>/dev/null), board: $(cat /sys/class/dmi/id/board_name 2>/dev/null)"
