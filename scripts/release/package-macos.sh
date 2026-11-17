#!/usr/bin/env bash
# Builds a macOS release of box-engine, locally only (docs/RELEASING.md):
#
#   target/release-dist/box-engine-<version>-macos/      the release folder
#   target/release-dist/box-engine-<version>-macos.zip   the same, zipped
#   target/release-dist/box-engine-<version>-macos.dmg   when hdiutil works
#
# Nothing is published, tagged or uploaded, and nothing is signed with a real
# identity: the app and its programs are ad-hoc signed, not notarised.
#
# Usage: scripts/release/package-macos.sh
#
# Slots filled from outside when present (each is reported either way):
#   target/templates/windows/box-player.exe   Windows player template, built on
#                                             Windows by CI (docs/EXPORT.md §6)
#   templates/projects/                       project templates for `box new`
#                                             and the editor's New Project
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
cd "$root"

# Where the Windows player template is picked up (apps/editor/macos/bundle.sh
# reads the same path) and where project templates come from. Both land in
# Box Editor.app/Contents/Resources/templates/ (windows/, projects/).
windows_template="$root/target/templates/windows/box-player.exe"
project_templates="$root/templates/projects"
# Script-only example projects shipped in the release folder's examples/
# (the guides refer to them as examples/<name>). Native ones need Rust.
examples=(guide_samples script_demo collector_luau cinder_keep script_rules)

[[ "$(uname -s)" == Darwin ]] || { echo "error: macOS releases are built on macOS" >&2; exit 1; }

version="$(cargo metadata --no-deps --format-version 1 \
  | python3 -I -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "box_cli"))')"
name="box-engine-$version-macos"
out="$root/target/release-dist"
dist="$out/$name"
echo "box-engine $version: packaging $dist"

cargo build --release -p box_cli -p box_editor_app -p box_player_app
# Box Editor.app with Resources/templates/{VERSION,macos,windows?} and the
# icon, ad-hoc signed.
apps/editor/macos/bundle.sh >/dev/null

rm -rf "$dist" "$out/$name.zip" "$out/$name.dmg"
mkdir -p "$dist"
app="$dist/Box Editor.app"
ditto "$root/target/release/Box Editor.app" "$app"
cp target/release/box "$app/Contents/MacOS/box"
templates="$app/Contents/Resources/templates"

if [[ -f "$templates/windows/box-player.exe" ]]; then
  echo "Windows player template: included (from $windows_template)"
else
  echo "Windows player template: not included ($windows_template is missing; CI builds it on Windows). This release exports macOS games only."
fi
if [[ -d "$project_templates" ]]; then
  ditto "$project_templates" "$templates/projects"
  echo "Project templates: included from $project_templates"
else
  echo "Project templates: none ($project_templates does not exist); \`box new\` makes its default project"
fi
# The editor's Open Example gallery: the list and previews, and each listed
# project (tracked files only, so no .box/ state except committed .box/types).
gallery="$root/templates/examples"
if [[ -f "$gallery/examples.json" ]]; then
  mkdir -p "$templates/examples"
  ditto "$gallery" "$templates/examples"
  for gallery_example in $(python3 -I -c 'import json,sys; print(" ".join(e["name"] for e in json.load(open(sys.argv[1]))["examples"]))' "$gallery/examples.json"); do
    [[ -d "examples/$gallery_example" ]] || { echo "error: gallery example examples/$gallery_example does not exist" >&2; exit 1; }
    git ls-files "examples/$gallery_example" | sed "s|^examples/$gallery_example/||" \
      | rsync -a --files-from=- "examples/$gallery_example/" "$templates/examples/$gallery_example/"
  done
  echo "Examples gallery: included ($(ls -d "$templates/examples"/*/ | wc -l | tr -d ' ') projects)"
else
  echo "Examples gallery: none ($gallery/examples.json does not exist)"
fi

# Sign the nested programs, then the bundle (signing seals Resources, so it is
# the last change to the app).
codesign --force --sign - --timestamp=none "$app/Contents/MacOS/box"
codesign --force --sign - --timestamp=none "$templates/macos/box-player"
codesign --force --sign - --timestamp=none "$app"
codesign --verify --strict --deep "$app"

# The commands, as links into the app: one copy of each program. bin/box-player
# is the macOS player template itself.
mkdir -p "$dist/bin"
ln -s "../Box Editor.app/Contents/MacOS/box" "$dist/bin/box"
ln -s "../Box Editor.app/Contents/MacOS/box-editor" "$dist/bin/box-editor"
ln -s "../Box Editor.app/Contents/Resources/templates/macos/box-player" "$dist/bin/box-player"

# Documentation and examples, as tracked in git (no .box/ state or builds).
mkdir -p "$dist/docs"
cp -R docs/guide docs/reference "$dist/docs/"
cp docs/TROUBLESHOOTING.md docs/STABILITY.md "$dist/docs/"
cp LICENSE "$dist/LICENSE.txt"
for example in "${examples[@]}"; do
  [[ -f "examples/$example/Cargo.toml" ]] && { echo "error: examples/$example is native" >&2; exit 1; }
  git ls-files "examples/$example" | rsync -a --files-from=- ./ "$dist/"
done

sed "s/@VERSION@/$version/g" "$here/macos/install.sh" > "$dist/install.sh"
chmod +x "$dist/install.sh"
sed "s/@VERSION@/$version/g" "$here/macos/README.txt" > "$dist/README.txt"

# The three programs report the same version as the workspace.
for tool in box box-editor box-player; do
  reported="$("$dist/bin/$tool" --version)"
  if [[ "$reported" != "$tool $version" ]]; then
    echo "error: $tool --version printed \`$reported\`, expected \`$tool $version\`" >&2
    exit 1
  fi
done
echo "Versions: box, box-editor and box-player report $version"

# ditto keeps the links and permissions, as Archive Utility unpacks them;
# extended attributes (provenance, ._ files) stay out.
(cd "$out" && ditto -c -k --norsrc --noextattr --keepParent "$name" "$name.zip")
if hdiutil create -quiet -volname "box-engine $version" -srcfolder "$dist" \
  -fs HFS+ -format UDZO -ov "$out/$name.dmg" 2>"$out/hdiutil.log"; then
  rm -f "$out/hdiutil.log"
  dmg="$out/$name.dmg"
else
  echo "warning: hdiutil could not make a disk image (see $out/hdiutil.log); the zip is the release" >&2
  dmg=""
fi

echo
du -sh "$dist" "$out/$name.zip" ${dmg:+"$dmg"}
echo "Not published: test it with scripts/release/clean-machine-test.sh $out/$name.zip"
