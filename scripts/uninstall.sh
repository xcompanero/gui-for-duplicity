#!/usr/bin/env bash
# Uninstall GUI For Duplicity (user-local install).
# Profiles in ~/.config/gui-for-duplicity and all backup data are untouched.
set -euo pipefail

BIN_DIR="${HOME}/.local/bin"
APP_DIR="${HOME}/.local/share/applications"
ICON_DIR="${HOME}/.local/share/icons/hicolor/128x128/apps"

rm -f "$BIN_DIR/gui-for-duplicity" "$BIN_DIR/engine-cli"
rm -f "$APP_DIR/gui-for-duplicity.desktop"
rm -f "$ICON_DIR/gui-for-duplicity.png"

# Remove any timers the tool created, best effort.
for unit in "${HOME}"/.config/systemd/user/gfd-*.timer; do
    [ -e "$unit" ] || continue
    systemctl --user disable --now "$(basename "$unit")" 2>/dev/null || true
    rm -f "$unit"
done
for unit in "${HOME}"/.config/systemd/user/gfd-*.service; do
    [ -e "$unit" ] || continue
    rm -f "$unit"
done
systemctl --user daemon-reload 2>/dev/null || true

echo "Uninstalled. Profiles remain in ~/.config/gui-for-duplicity/"
