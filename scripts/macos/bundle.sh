#!/usr/bin/env bash
# Build Mini Remote in release mode and put a fresh app in dist/Mini Remote.app.
# Run it after every change to the app so dist/ always holds the latest version.
# The app bundle is needed so macOS asks for, and remembers, permission to
# talk to devices on the local network (finding and connecting to a Pi).
#
# Saved connections and settings live in ~/Library/Application Support/Mini Remote
# and passwords in the Keychain, so rebuilding never touches them.
#
# Usage: scripts/macos/bundle.sh [--open]
#   Result: dist/Mini Remote.app   (open it from there, or copy it to /Applications)
#   If that app is running, it is closed while updating and opened again afterwards.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
cargo build --release -p mini-remote

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
app="$root/dist/Mini Remote.app"
staging="$root/dist/.Mini Remote.app.new"

# Assemble the new app next to the old one, so dist/ never holds a half-built app.
rm -rf "$staging"
mkdir -p "$staging/Contents/MacOS" "$staging/Contents/Resources"
cp "$target_dir/release/mini-remote" "$staging/Contents/MacOS/mini-remote"
sed "s/__VERSION__/$version/g" scripts/macos/Info.plist > "$staging/Contents/Info.plist"
if [ ! -f assets/AppIcon.icns ]; then
    python3 scripts/macos/make-icon.py
fi
cp assets/AppIcon.icns "$staging/Contents/Resources/AppIcon.icns"

# Ad-hoc signature so macOS keeps the network permission between launches.
codesign --force --deep --sign - "$staging" >/dev/null

# Close the app from dist/ if it is running, swap in the new one, then reopen it.
was_running=false
if pgrep -f "$app/Contents/MacOS/mini-remote" >/dev/null; then
    was_running=true
    osascript -e 'tell application id "app.mini-remote.viewer" to quit' >/dev/null 2>&1 || true
    for _ in $(seq 1 50); do
        pgrep -f "$app/Contents/MacOS/mini-remote" >/dev/null || break
        sleep 0.1
    done
    pkill -f "$app/Contents/MacOS/mini-remote" 2>/dev/null || true
fi
rm -rf "$app"
mv "$staging" "$app"

echo "Built dist/Mini Remote.app (v$version)"
if [ "$was_running" = true ] || [ "${1:-}" = "--open" ]; then
    open "$app"
fi
