#!/usr/bin/env bash
# Clean-machine test of a macOS release (docs/RELEASING.md): installs the zip
# into a fresh HOME with no Rust toolchain on PATH, then follows
# docs/guide/getting-started.md with the installed programs only: new project,
# the guide's first script, check, run, render, export, the exported game run
# from another folder, and the editor.
#
# Usage: scripts/release/clean-machine-test.sh [release.zip]
#   (default: the newest target/release-dist/box-engine-*-macos.zip)
#
# BOX_TEST_TEMPLATE=<name>  create the project with `box new --template <name>`
# BOX_CLEAN_MACHINE_NO_GPU=1  skip the steps that start a GPU device (`box run`,
#                           `box render`, the exported game's runs, the editor
#                           capture): for a machine without a usable Metal
#                           device (the release workflow's fallback on a
#                           hosted runner). Every other step still runs.
#
# Every step runs under `env -i` with HOME set to a new folder and PATH set to
# the system folders plus the install's ~/.local/bin. The checks around the
# steps (PNG contents, linked libraries) use the host's python3 and otool.
# Captures are copied to target/captures/clean-machine/.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
zip="${1:-$(ls -t "$root"/target/release-dist/box-engine-*-macos.zip 2>/dev/null | head -1)}"
[[ -f "$zip" ]] || { echo "error: no release zip (run scripts/release/package-macos.sh)" >&2; exit 1; }
zip="$(cd "$(dirname "$zip")" && pwd)/$(basename "$zip")"
name="$(basename "$zip" .zip)"
version="${name#box-engine-}"
version="${version%-macos}"

out="$root/target/captures/clean-machine"
rm -rf "$out"
work="$(mktemp -d "${TMPDIR:-/tmp}/box-clean-machine.XXXXXX")"
work="$(cd "$work" && pwd -P)"
home="$work/home"
captures="$work/captures"
mkdir -p "$home" "$work/tmp" "$captures"
clean_path="$home/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
keep_work=1
trap '[[ $keep_work == 1 ]] && echo "Left the test folder for inspection: $work" >&2' EXIT

# Runs a command as on a machine with nothing but macOS and this release.
clean() {
  env -i HOME="$home" PATH="$clean_path" TMPDIR="$work/tmp" USER="${USER:-tester}" \
    LOGNAME="${USER:-tester}" LANG=en_US.UTF-8 SHELL=/bin/zsh "$@"
}

now_ms() { /usr/bin/perl -MTime::HiRes=time -e 'printf "%.0f", time * 1000'; }
timings=""
step() {
  local label="$1"
  shift
  echo
  echo "== $label"
  local start end
  start="$(now_ms)"
  "$@"
  end="$(now_ms)"
  timings="$timings$(printf '%-44s %7d ms' "$label" $((end - start)))"$'\n'
}
fail() { echo "FAIL: $*" >&2; exit 1; }
not_blank() { python3 -I "$here/png_check.py" blank "$1" || fail "$1 is blank"; }
gpu=1
[[ "${BOX_CLEAN_MACHINE_NO_GPU:-}" == 1 ]] && gpu=0
# Like `step`, for a step that needs a GPU: reported as skipped without one.
gpu_step() {
  if [[ $gpu == 1 ]]; then
    step "$@"
  else
    echo
    echo "== $1: skipped (BOX_CLEAN_MACHINE_NO_GPU=1)"
    skipped="$skipped $1;"
  fi
}
skipped=""

echo "Release: $zip"
echo "Clean HOME: $home"

no_rust() {
  for tool in cargo rustc rustup; do
    if clean /bin/sh -c "command -v $tool"; then
      fail "$tool is on the clean PATH"
    fi
  done
  [[ ! -e "$home/.cargo" && ! -e "$home/.rustup" ]] || fail "the clean HOME has Rust"
  echo "cargo, rustc and rustup: not found"
}
step "No Rust toolchain" no_rust

unpack() { clean /usr/bin/ditto -x -k "$zip" "$home/Downloads"; }
step "Unpack the zip" unpack
release="$home/Downloads/$name"

install() { (cd "$release" && clean ./install.sh); }
step "install.sh" install
app="$home/Applications/Box Editor.app"
[[ -d "$app" ]] || fail "install.sh did not install $app"

versions() {
  for tool in box box-editor box-player; do
    local reported
    reported="$(clean "$tool" --version)"
    echo "$reported"
    [[ "$reported" == "$tool $version" ]] || fail "$tool --version: \`$reported\`, expected \`$tool $version\`"
    [[ "$(clean /bin/sh -c "command -v $tool")" == "$home/.local/bin/$tool" ]] || fail "$tool is not the installed one"
  done
  # Only system libraries: nothing from a toolchain or Homebrew.
  if command -v otool >/dev/null; then
    for exe in "$app/Contents/MacOS/box" "$app/Contents/MacOS/box-editor" \
      "$app/Contents/Resources/templates/macos/box-player"; do
      if otool -L "$exe" | tail -n +2 | awk '{print $1}' | grep -vE '^(/usr/lib/|/System/Library/)'; then
        fail "$exe links a library outside macOS"
      fi
    done
    echo "linked libraries: macOS only"
  fi
  codesign --verify --strict --deep "$app"
  echo "Box Editor.app: signature valid (ad-hoc)"
}
step "Versions, libraries, signature" versions

projects="$home/Projects"
mkdir -p "$projects"
in_projects() { (cd "$projects" && clean "$@"); }

new_project() {
  if [[ -n "${BOX_TEST_TEMPLATE:-}" ]]; then
    in_projects box new --template "$BOX_TEST_TEMPLATE" my-game
  else
    in_projects box new my-game
  fi
}
step "box new my-game" new_project

# The editor's Open Example gallery ships inside the app, with every listed project.
gallery() {
  local dir="$app/Contents/Resources/templates/examples"
  [[ -f "$dir/examples.json" ]] || fail "no examples gallery in $dir"
  local n
  for n in $(python3 -I -c 'import json,sys; print(" ".join(e["name"] for e in json.load(open(sys.argv[1]))["examples"]))' "$dir/examples.json"); do
    [[ -f "$dir/$n/box.project.json" ]] || fail "gallery example $n is missing from $dir"
  done
  echo "gallery: $(ls -d "$dir"/*/ | wc -l | tr -d ' ') projects"
}
step "Examples gallery in the app" gallery

# The no-code path: the First Game example (presets only) from the gallery,
# as Open Example copies it: it checks without warnings, plays and exports.
first="$projects/first-game"
first_game_copy() {
  ditto "$app/Contents/Resources/templates/examples/first_game" "$first"
  local out
  out="$(in_projects box check first-game 2>&1)"
  echo "$out" | tail -1
  echo "$out" | tail -1 | grep -q ' 0 error(s), 0 warning(s)' || fail "first_game does not check clean: $out"
}
step "First Game example: box check" first_game_copy
gpu_step "First Game example: box run --frames 120" in_projects box run first-game --frames 120
first_game_export() {
  in_projects box export --project first-game --target macos --profile release --out first-game/export
  ls -d "$first"/export/*.app >/dev/null 2>&1 || fail "no .app in $first/export"
}
step "First Game example: box export" first_game_export
game="$projects/my-game"

# The guide's first script and its `box scene apply` commands, copied from the
# getting-started guide shipped in the release, so the test follows the text.
guide="$release/docs/guide/getting-started.md"
write_script() {
  mkdir -p "$game/assets/scripts"
  awk '/^## Write your first script/ {s = 1} s && /^```lua$/ {c = 1; next} c && /^```$/ {exit} c' \
    "$guide" > "$game/assets/scripts/spinner.luau"
  grep -q 'rotate_y' "$game/assets/scripts/spinner.luau" || fail "no spinner script in $guide"
  awk "/^cat > spin.json <<'EOF'\$/ {c = 1; next} c && /^EOF\$/ {exit} c" "$guide" > "$projects/spin.json"
  grep -q '"add_component"' "$projects/spin.json" || fail "no spin.json in $guide"
  cat "$game/assets/scripts/spinner.luau"
  in_projects box scene apply my-game/assets/scenes/main.scene.json spin.json
}
if [[ -n "${BOX_TEST_TEMPLATE:-}" ]]; then
  # A template's scene has no `Cube`: the guide's commands are for the default project.
  echo "== Write the guide's script: skipped (template $BOX_TEST_TEMPLATE)"
else
  step "Write the guide's script, attach it" write_script
fi

step "box check" in_projects box check my-game
gpu_step "box run --frames 120" in_projects box run my-game --frames 120
render() {
  in_projects box render my-game --out "$captures/render.png" --size 1280x720
  not_blank "$captures/render.png"
}
gpu_step "box render" render

export_macos() {
  in_projects box export --project my-game --target macos --profile release --out my-game/export
}
step "box export --target macos --profile release" export_macos
exported="$(ls -d "$game"/export/*.app 2>/dev/null | head -1 || true)"
[[ -d "$exported" ]] || fail "no .app in $game/export"

# The exported game, moved away from the project and the engine, as a player
# would get it.
shipped="$home/Desktop"
ship_exported() {
  mkdir -p "$shipped"
  ditto "$exported" "$shipped/$(basename "$exported")"
  codesign --verify --strict "$shipped/$(basename "$exported")"
  echo "$(basename "$exported"): copied to another folder, signature valid"
}
step "Copy the exported game away" ship_exported
run_exported() {
  local bin
  bin="$shipped/$(basename "$exported")/Contents/MacOS/$(basename "$exported" .app)"
  (cd "$shipped" && clean "$bin" --frames 5 --size 1280x720 --capture "$captures/exported_f5.png")
  (cd "$shipped" && clean "$bin" --frames 35 --size 1280x720 --capture "$captures/exported_f35.png")
  not_blank "$captures/exported_f5.png"
  not_blank "$captures/exported_f35.png"
  # The spinner turns the cube between frames 5 and 35 (45 degrees): the script is in the pak.
  if [[ -z "${BOX_TEST_TEMPLATE:-}" ]]; then
    python3 -I "$here/png_check.py" differ "$captures/exported_f5.png" "$captures/exported_f35.png" \
      || fail "the exported game's frames 5 and 35 are equal: the script did not run"
  fi
}
gpu_step "Run the exported game (headless)" run_exported

windows_template="$app/Contents/Resources/templates/windows/box-player.exe"
if [[ -f "$windows_template" ]]; then
  export_windows() {
    in_projects box export --project my-game --target windows --profile release --out my-game/export-windows
    ls "$game/export-windows"/*/*.exe "$game/export-windows"/*/game.pak
  }
  step "box export --target windows" export_windows
else
  echo
  echo "== box export --target windows: skipped (this release has no Windows player template)"
fi

editor() {
  in_projects box-editor my-game --capture "$captures/editor.png"
  not_blank "$captures/editor.png"
}
gpu_step "box-editor --capture" editor

uninstall() {
  (cd "$release" && clean ./install.sh --uninstall)
  [[ ! -e "$app" ]] || fail "uninstall left $app"
  for tool in box box-editor box-player; do
    [[ ! -e "$home/.local/bin/$tool" && ! -L "$home/.local/bin/$tool" ]] || fail "uninstall left $tool"
  done
  [[ -f "$game/box.project.json" ]] || fail "uninstall touched the project"
}
step "install.sh --uninstall" uninstall

mkdir -p "$out"
if compgen -G "$captures/*.png" >/dev/null; then
  cp "$captures"/*.png "$out/"
fi
keep_work=0
rm -rf "$work"

echo
if [[ $gpu == 1 ]]; then
  echo "Clean-machine test passed: box-engine $version"
else
  echo "Clean-machine test passed without the GPU steps: box-engine $version"
  echo "Not run (BOX_CLEAN_MACHINE_NO_GPU=1):$skipped"
fi
printf '%s' "$timings"
echo "Captures: $out"
