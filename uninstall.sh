#!/usr/bin/env bash
#
# Reverse install.sh. Leaves nothing behind but transcripts you saved yourself.
set -euo pipefail

APP_ID="us.hagreli.Remarkable"
PREFIX="${PREFIX:-$HOME/.local}"
DATA_DIR="$PREFIX/share"

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

say "Removing from $PREFIX"
rm -f "$PREFIX/bin/remarkable"
rm -f "$DATA_DIR/applications/$APP_ID.desktop"
rm -f "$DATA_DIR/metainfo/$APP_ID.metainfo.xml"
rm -f "$DATA_DIR/icons/hicolor/scalable/apps/$APP_ID.svg"
rm -f "$DATA_DIR/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"
rm -f "$DATA_DIR/dbus-1/services/$APP_ID.service"
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/remarkable"

if command -v gtk4-update-icon-cache >/dev/null; then
  gtk4-update-icon-cache -qtf "$DATA_DIR/icons/hicolor" 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null; then
  update-desktop-database -q "$DATA_DIR/applications" 2>/dev/null || true
fi

say "Removed."
