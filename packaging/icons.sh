#!/usr/bin/env bash
# Regenerate every app icon file from assets/app-icon/lightcraft.svg.
#
# Needs: resvg (brew install resvg / cargo install resvg). On macOS, iconutil also writes the
# .icns. The outputs are committed, so building and packaging never need these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/lightcraft.svg"
ID="ai.storyteller.lightcraft"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

# The SVG is the full-bleed 512 tile (rx=112). Windows and Linux use it as is, so the lynx reads at
# 16-48 px. macOS icons follow Apple's grid: an 824/1024 body with a transparent margin, made by
# widening the viewBox (512 / 0.805 = 636, so 62 units each side).
MAC="$TMP/macos.svg"
sed 's/viewBox="0 0 512 512"/viewBox="-62 -62 636 636"/' "$SVG" >"$MAC"
grep -q 'viewBox="-62 -62 636 636"' "$MAC" || { echo "error: unexpected viewBox in $SVG" >&2; exit 1; }

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

render "$SVG" 1024 "$DIR/lightcraft-1024.png"
# Runtime window/Dock icon on macOS (embedded by apps/lightcraft/src/main.rs).
render "$MAC" 512 "$DIR/lightcraft-macos-512.png"

# Linux hicolor theme (also the runtime window icon on Windows and Linux: 256x256).
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$DIR/lightcraft-small.svg" "$DIR/hicolor/scalable/apps/$ID.svg"

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$SVG" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- ico "$DIR/lightcraft.ico" "${ICO_PNGS[@]}")

# macOS .icns.
if command -v iconutil >/dev/null; then
  SET="$TMP/lightcraft.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    render "$MAC" "$s" "$SET/icon_${s}x${s}.png"
    render "$MAC" $((s * 2)) "$SET/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns -o "$DIR/lightcraft.icns" "$SET"
else
  echo "warning: iconutil not found (macOS only); lightcraft.icns not regenerated" >&2
fi
echo "icons written to $DIR"
