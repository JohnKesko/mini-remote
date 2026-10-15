#!/usr/bin/env bash
# Build Mini Remote in release mode and wrap it in a macOS app (Mini Remote.app).
# The app bundle is needed so macOS asks for, and remembers, permission to
# talk to devices on the local network (finding and connecting to a Pi).
#
# Usage: scripts/macos/bundle.sh [--open]
#   Result: target/release/Mini Remote.app   (copy it to /Applications if you like)
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
cargo build --release -p mini-remote

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
app="$target_dir/release/Mini Remote.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

cp "$target_dir/release/mini-remote" "$app/Contents/MacOS/mini-remote"
sed "s/__VERSION__/$version/g" scripts/macos/Info.plist > "$app/Contents/Info.plist"
if [ ! -f assets/AppIcon.icns ]; then
    python3 scripts/macos/make-icon.py
fi
cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"

# Ad-hoc signature so macOS keeps the network permission between launches.
codesign --force --deep --sign - "$app" >/dev/null

echo "Built $app"
if [ "${1:-}" = "--open" ]; then
    open "$app"
fi
