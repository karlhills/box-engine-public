#!/usr/bin/env bash
# Builds tiny.exe, a Windows x86-64 console PE, on macOS without a Windows
# toolchain: rustc's Windows target (rust-std only) and its bundled rust-lld.
#   rustup target add x86_64-pc-windows-msvc
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
rustc --target x86_64-pc-windows-msvc -O -C panic=abort -C linker=rust-lld \
  -C link-arg=/SUBSYSTEM:CONSOLE -C link-arg=/NODEFAULTLIB -C link-arg=/ENTRY:mainCRTStartup \
  "$here/tiny.rs" -o "$here/tiny.exe"
echo "$here/tiny.exe"
