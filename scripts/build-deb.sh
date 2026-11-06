#!/usr/bin/env bash
# Build a Debian package (.deb) containing the release binaries, desktop
# entry and icon. Output: dist/gui-for-duplicity_<version>_amd64.deb
#
# Target systems: Ubuntu/Debian with a matching glibc (built on Kubuntu
# 26.04). Runtime dependencies are declared in the control file; on the
# target machine install with:
#   sudo apt install ./gui-for-duplicity_<version>_amd64.deb
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
ARCH="amd64"
STAGING="target/deb/gui-for-duplicity"
OUT_DIR="dist"

for binary in target/release/gui-for-duplicity target/release/engine-cli; do
    [ -x "$binary" ] || { echo "Missing $binary — run 'cargo build --release' first." >&2; exit 1; }
done

# Assemble the package tree (filesystem layout inside the deb).
rm -rf "$STAGING"
mkdir -p "$STAGING/DEBIAN" \
         "$STAGING/usr/bin" \
         "$STAGING/usr/share/applications" \
         "$STAGING/usr/share/icons/hicolor/128x128/apps" \
         "$STAGING/usr/share/doc/gui-for-duplicity"

install -m 755 target/release/gui-for-duplicity "$STAGING/usr/bin/"
install -m 755 target/release/engine-cli "$STAGING/usr/bin/"
install -m 644 packaging/gui-for-duplicity.desktop "$STAGING/usr/share/applications/"
install -m 644 app/src-tauri/icons/icon.png "$STAGING/usr/share/icons/hicolor/128x128/apps/gui-for-duplicity.png"
install -m 644 LICENSE "$STAGING/usr/share/doc/gui-for-duplicity/copyright"

cat > "$STAGING/DEBIAN/control" <<EOF
Package: gui-for-duplicity
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Depends: duplicity, libwebkit2gtk-4.1-0, libgtk-3-0t64, libnotify-bin
Recommends: openssh-client
Installed-Size: $(du -sk "$STAGING/usr" | cut -f1)
Maintainer: Aidan <aidan@localhost>
Homepage: https://github.com/TODO/gui-for-duplicity
Description: Desktop GUI front end for the Duplicity backup program
 GUI For Duplicity is a Rust front end for the Duplicity backup engine:
 profiles with scheduled (systemd user timers) and manual backups, live
 verbose logs, snapshot browsing (current and historical), safe restores,
 full data verification, retention housekeeping and desktop notifications.
 .
 All duplicity knowledge lives in an engine library shared by the GUI and
 the headless engine-cli; secrets are stored in the desktop secret service
 (KWallet) and passed to duplicity via the environment only.
EOF

# Build it.
mkdir -p "$OUT_DIR"
dpkg-deb --build --root-owner-group "$STAGING" "$OUT_DIR/gui-for-duplicity_${VERSION}_${ARCH}.deb"

echo
echo "Built: $OUT_DIR/gui-for-duplicity_${VERSION}_${ARCH}.deb"
echo "Install on the target machine with:"
echo "  sudo apt install ./$OUT_DIR/gui-for-duplicity_${VERSION}_${ARCH}.deb"
