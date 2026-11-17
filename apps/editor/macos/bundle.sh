#!/usr/bin/env bash
# Builds the release editor and wraps it in a macOS app bundle:
#   target/release/Box Editor.app
# Usage: apps/editor/macos/bundle.sh [--open]
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
cd "$root"

# The editor and the player template File ▸ Export… bundles into games.
cargo build --release -p box_editor_app -p box_player_app

version="$(cargo metadata --no-deps --format-version 1 \
  | python3 -I -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "box_editor_app"))')"

app="$root/target/release/Box Editor.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/box-editor "$app/Contents/MacOS/box-editor"
sed "s/@VERSION@/$version/g" "$here/Info.plist" > "$app/Contents/Info.plist"

# Player templates (box_export::template): the macOS one built here; a
# Windows one only when CI built it on Windows and left it in
# target/templates/windows/.
templates="$app/Contents/Resources/templates"
mkdir -p "$templates/macos"
cp target/release/box-player "$templates/macos/box-player"
if [[ -f "$root/target/templates/windows/box-player.exe" ]]; then
  mkdir -p "$templates/windows"
  cp "$root/target/templates/windows/box-player.exe" "$templates/windows/box-player.exe"
fi
printf '%s\n' "$version" > "$templates/VERSION"

iconset="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$here/AppIcon.png" --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" "$here/AppIcon.png" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$(dirname "$iconset")"

# Ad-hoc signature so Apple Silicon runs the bundle; not notarised.
codesign --force --sign - "$app"

echo "$app"
if [[ "${1:-}" == "--open" ]]; then
  open "$app"
fi
