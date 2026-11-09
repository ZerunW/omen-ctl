#!/usr/bin/env python3
"""Write the licence notices of every Rust crate built into the omenctl binaries.

Usage: third-party-licenses.py <cargo> <app dir> <output file>

Reads `cargo metadata` for x86_64 Linux (the only target the .deb is built for),
then for each third-party crate copies the licence/notice files shipped inside
the crate. Crates whose archive has no licence file get the standard text of
their licence appended once at the end (MIT needs the copyright line, which is
taken from the crate's authors).
"""
import glob
import json
import os
import subprocess
import sys

cargo, app, out = sys.argv[1:4]
meta = json.loads(subprocess.run(
    [cargo, "metadata", "--format-version", "1", "--locked",
     "--filter-platform", "x86_64-unknown-linux-gnu"],
    cwd=app, check=True, capture_output=True, text=True).stdout)

used = {n["id"] for n in meta["resolve"]["nodes"]}
crates = sorted((p for p in meta["packages"] if p["id"] in used and p["source"] is not None),
                key=lambda p: (p["name"], p["version"]))

PREFIXES = ("LICEN", "COPYING", "NOTICE", "COPYRIGHT", "UNLICENSE")
standard = {}  # licence id -> text, from crates that ship one
bare = []
parts = []
for p in crates:
    d = os.path.dirname(p["manifest_path"])
    files = sorted(f for f in glob.glob(os.path.join(d, "*"))
                   if os.path.isfile(f) and os.path.basename(f).upper().startswith(PREFIXES))
    # bundled fonts carry their licences next to them (epaint_default_fonts)
    files += sorted(glob.glob(os.path.join(d, "fonts", "*.txt")))
    head = f"{p['name']} {p['version']}\nLicence: {p['license'] or p.get('license_file')}\n"
    if p.get("repository"):
        head += f"Source: {p['repository']}\n"
    if p.get("authors"):
        head += "Authors: " + ", ".join(p["authors"]) + "\n"
    body = ""
    for f in files:
        with open(f, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        body += f"\n--- {os.path.basename(f)} ---\n{text.rstrip()}\n"
        name = os.path.basename(f).upper()
        if "APACHE" in name and "Apache-2.0" not in standard:
            standard["Apache-2.0"] = text
    if not files:
        bare.append(p)
        body = "\n(No licence file in the crate archive; the standard text is at the end of this file.)\n"
    parts.append("=" * 78 + "\n" + head + body)

MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in the
Software without restriction, including without limitation the rights to use, copy,
modify, merge, publish, distribute, sublicense, and/or sell copies of the Software,
and to permit persons to whom the Software is furnished to do so, subject to the
following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT
HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF
CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE
OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
"""

tail = ""
if bare:
    tail += "=" * 78 + "\nStandard licence texts for the crates above without a licence file\n\n"
    tail += "MIT licence. For each crate below: Copyright (c) the crate's authors listed above.\n\n"
    tail += MIT + "\n"
    if "Apache-2.0" in standard:
        tail += "-" * 78 + "\nApache License 2.0\n\n" + standard["Apache-2.0"].rstrip() + "\n"
    else:
        sys.exit("no Apache-2.0 text found in any crate")

with open(out, "w", encoding="utf-8") as fh:
    fh.write("Third-party software in the omenctl binaries (omenctld, omenctl, omenctl-gui)\n\n")
    fh.write("The omenctl code is GPL-2.0-or-later. Some crates used by the desktop app\n")
    fh.write("(winit, glutin and others) are Apache-2.0 only, which is compatible with GPL\n")
    fh.write("version 3 but not version 2; omenctl-gui as a whole is therefore distributed\n")
    fh.write("under GPL-3.0-or-later. The kernel driver contains none of these crates.\n")
    fh.write("The app embeds fonts: IBM Plex Sans (SIL OFL 1.1) and egui's default fonts\n")
    fh.write("(epaint_default_fonts: OFL-1.1 and Ubuntu Font Licence 1.0).\n\n")
    fh.write(f"{len(crates)} crates.\n\n")
    fh.write("".join(parts))
    fh.write(tail)
print(f"{out}: {len(crates)} crates, {len(bare)} without a licence file")
