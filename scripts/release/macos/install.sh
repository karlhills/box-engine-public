#!/bin/bash
# Installs box-engine @VERSION@ from this folder, without sudo:
#   Box Editor.app                 -> ~/Applications/Box Editor.app
#   box, box-editor, box-player    -> ~/.local/bin (links into the app)
#
# Usage: ./install.sh [--app-dir <dir>] [--bin-dir <dir>]
#        ./install.sh --uninstall [--app-dir <dir>] [--bin-dir <dir>]
#
#   --app-dir <dir>   Install the app here instead (for example /Applications,
#                     for every user of this Mac, if you may write to it)
#   --bin-dir <dir>   Put the command links here instead of ~/.local/bin
#   --uninstall       Remove the app and the links this script made
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
app_dir="$HOME/Applications"
bin_dir="$HOME/.local/bin"
uninstall=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --app-dir) app_dir="${2:?--app-dir needs a folder}"; shift 2 ;;
    --bin-dir) bin_dir="${2:?--bin-dir needs a folder}"; shift 2 ;;
    --uninstall) uninstall=1; shift ;;
    -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "error: unknown option $1 (--help lists them)" >&2; exit 1 ;;
  esac
done

# Links must be absolute: resolve the folders given.
absolute() { if [[ -d "$1" ]]; then (cd "$1" && pwd); else echo "$1"; fi; }
[[ $uninstall == 1 ]] || mkdir -p "$app_dir" "$bin_dir"
app_dir="$(absolute "$app_dir")"
bin_dir="$(absolute "$bin_dir")"
app="$app_dir/Box Editor.app"
tools="box box-editor box-player"

# Where each command's link points, inside the installed app.
link_target() {
  case "$1" in
    box-player) echo "$app/Contents/Resources/templates/macos/box-player" ;;
    *) echo "$app/Contents/MacOS/$1" ;;
  esac
}

if [[ $uninstall == 1 ]]; then
  for tool in $tools; do
    link="$bin_dir/$tool"
    if [[ -L "$link" && "$(readlink "$link")" == "$(link_target "$tool")" ]]; then
      rm "$link"
      echo "Removed $link"
    fi
  done
  if [[ -d "$app" ]]; then
    if [[ "$(defaults read "$app/Contents/Info" CFBundleIdentifier 2>/dev/null)" != dev.box-engine.editor ]]; then
      echo "error: $app is not Box Editor; left in place" >&2
      exit 1
    fi
    rm -rf "$app"
    echo "Removed $app"
  fi
  echo "box-engine is uninstalled. Your projects, and the editor's settings in"
  echo "~/Library/Application Support/box-editor and logs in ~/Library/Logs, are kept."
  exit 0
fi

[[ -d "$here/Box Editor.app" ]] || { echo "error: run install.sh from the unpacked release folder" >&2; exit 1; }
[[ "$(uname -m)" == arm64 ]] || echo "warning: this release is built for Apple Silicon Macs" >&2

[[ -w "$app_dir" ]] || { echo "error: cannot write to $app_dir (choose another with --app-dir)" >&2; exit 1; }
if [[ -d "$app" ]]; then
  echo "Replacing $app"
  rm -rf "$app"
fi
ditto "$here/Box Editor.app" "$app"
# A release downloaded with a browser is quarantined, and Gatekeeper refuses
# quarantined apps that are not notarised; this release is ad-hoc signed.
xattr -dr com.apple.quarantine "$app" 2>/dev/null || true
echo "Installed $app"

for tool in $tools; do
  link="$bin_dir/$tool"
  if [[ -e "$link" && ! -L "$link" ]]; then
    echo "error: $link exists and is not a link; remove it or choose another --bin-dir" >&2
    exit 1
  fi
  ln -sfn "$(link_target "$tool")" "$link"
done
echo "Linked box, box-editor and box-player in $bin_dir"
"$bin_dir/box" --version

case ":$PATH:" in
  *":$bin_dir:"*)
    found="$(command -v box || true)"
    if [[ "$found" != "$bin_dir/box" ]]; then
      echo "note: \`box\` on your PATH is $found, which comes before $bin_dir"
    fi
    ;;
  *)
    echo
    echo "$bin_dir is not on your PATH. Add it (zsh, the macOS default shell):"
    echo "  echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.zprofile && source ~/.zprofile"
    ;;
esac
echo
echo "Next: box new my-game && box-editor my-game   (guide: docs/guide/getting-started.md)"
echo "Uninstall: ./install.sh --uninstall in this folder, or: rm -rf \"$app\" && rm $bin_dir/box $bin_dir/box-editor $bin_dir/box-player"
