#!/bin/sh
# Build omenctl_<version>_amd64.deb with plain dpkg-deb (no debhelper needed).
# Usage: packaging/build-deb.sh            (run from anywhere, as a normal user)
#        CARGO=cargo-1.97 packaging/build-deb.sh   (Ubuntu's versioned cargo package)
# Output: packaging/out/omenctl_<version>_amd64.deb
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
TOP=$(dirname "$HERE")
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$TOP/app/Cargo.toml" | head -n1)
ARCH=$(dpkg --print-architecture)
OUT="$HERE/out"
PKG="$OUT/omenctl_${VERSION}_${ARCH}"

echo "== building omenctl $VERSION ($ARCH)"
CARGO=${CARGO:-$(command -v cargo || command -v cargo-1.97 || echo cargo)}
(cd "$TOP/app" && "$CARGO" build --release --locked)
BIN="$TOP/app/target/release"

rm -rf "$PKG"
install -d "$PKG/DEBIAN"

# programs
install -D -m755 "$BIN/omenctld" "$PKG/usr/sbin/omenctld"
install -D -m755 "$BIN/omenctl" "$PKG/usr/bin/omenctl"
install -D -m755 "$BIN/omenctl-gui" "$PKG/usr/bin/omenctl-gui"
strip "$PKG/usr/sbin/omenctld" "$PKG/usr/bin/omenctl" "$PKG/usr/bin/omenctl-gui"

# helpers, units, policy, desktop entry, example config
install -D -m755 "$HERE/lib/omenctl-check" "$PKG/usr/lib/omenctl/omenctl-check"
install -D -m755 "$HERE/lib/omenctl-safe-exit" "$PKG/usr/lib/omenctl/omenctl-safe-exit"
install -D -m644 "$HERE/systemd/omenctld.service" "$PKG/usr/lib/systemd/system/omenctld.service"
install -D -m644 "$HERE/systemd/omenctl-nvidia-powerd.service" "$PKG/usr/lib/systemd/system/omenctl-nvidia-powerd.service"
install -D -m644 "$HERE/systemd/omenctl-nvidia-powerd.conf" "$PKG/usr/share/dbus-1/system.d/omenctl-nvidia-powerd.conf"
install -D -m644 "$HERE/omenctl-gui.desktop" "$PKG/usr/share/applications/omenctl-gui.desktop"
install -D -m644 "$HERE/config.toml" "$PKG/usr/share/omenctl/config.toml"

# DKMS source of the patched hp-wmi
SRC="$PKG/usr/src/omenctl-hp-wmi-$VERSION"
install -D -m644 "$TOP/hpwmi-upstream/src/hp-wmi.c" "$SRC/hp-wmi.c"
install -D -m644 "$TOP/hpwmi-upstream/src/Makefile" "$SRC/Makefile"
sed "s/@VERSION@/$VERSION/" "$HERE/dkms/dkms.conf" > "$SRC/dkms.conf"
chmod 644 "$SRC/dkms.conf"

# docs
DOC="$PKG/usr/share/doc/omenctl"
install -D -m644 "$TOP/docs/USER_GUIDE.md" "$DOC/USER_GUIDE.md"
install -D -m644 "$TOP/docs/HARDWARE_TEST.md" "$DOC/HARDWARE_TEST.md"
install -D -m644 "$TOP/README.md" "$DOC/README.md"
install -D -m644 "$TOP/hpwmi-upstream/backport.patch" "$DOC/hp-wmi-backport.patch"
install -D -m644 "$TOP/app/omenctl-gui/assets/IBMPlexSans-LICENSE.txt" "$DOC/IBMPlexSans-LICENSE.txt"
if [ -e "$TOP/LICENSE" ]; then install -m644 "$TOP/LICENSE" "$DOC/copyright"; fi

# maintainer scripts
for s in postinst prerm postrm; do
	sed "s/@VERSION@/$VERSION/" "$HERE/debian/$s" > "$PKG/DEBIAN/$s"
	chmod 755 "$PKG/DEBIAN/$s"
done

SIZE=$(du -sk "$PKG" | cut -f1)
cat > "$PKG/DEBIAN/control" <<EOF
Package: omenctl
Version: $VERSION
Architecture: $ARCH
Maintainer: Zerun Wang <213201277+ZerunW@users.noreply.github.com>
Installed-Size: $SIZE
Depends: dkms, libc6, libgcc-s1, libgl1, libxkbcommon0, libxkbcommon-x11-0, adduser
Recommends: fonts-noto-cjk, linux-headers-generic-hwe-24.04
Section: utils
Priority: optional
Homepage: https://github.com/ZerunW/omen-ctl
Description: performance and fan control for HP OMEN 16 (board 8D3F)
 Daemon (omenctld), command line tool (omenctl) and desktop app (omenctl-gui)
 that switch between power saver, balanced and unleashed modes and drive the
 fans with a curve. Includes a patched hp-wmi kernel driver built with DKMS
 (fan watchdog, CPU+GPU concurrent power target) and enables nvidia-powerd.
EOF

dpkg-deb --root-owner-group --build "$PKG" "$OUT/omenctl_${VERSION}_${ARCH}.deb"
rm -rf "$PKG"
echo "== done: $OUT/omenctl_${VERSION}_${ARCH}.deb"
