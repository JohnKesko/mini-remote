#!/bin/sh
# Installs Mini Remote for the current user only, so no admin password is
# needed and updates can replace the app in place.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin_dir="$HOME/.local/bin"
data_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
apps_dir="$data_dir/applications"
icons_dir="$data_dir/icons/hicolor/256x256/apps"

mkdir -p "$bin_dir" "$apps_dir" "$icons_dir"
install -m 755 "$here/mini-remote" "$bin_dir/mini-remote"
install -m 644 "$here/mini-remote.png" "$icons_dir/mini-remote.png"
# The menu entry points straight at the app, in case ~/.local/bin is not on
# the PATH until the next log in.
sed "s|^Exec=.*|Exec=\"$bin_dir/mini-remote\"|" "$here/mini-remote.desktop" > "$apps_dir/mini-remote.desktop"
chmod 644 "$apps_dir/mini-remote.desktop"

# Let the desktop notice the new menu entry and icon right away.
command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps_dir" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t "$data_dir/icons/hicolor" >/dev/null 2>&1 || true

echo "Mini Remote is installed. Open it from the app menu, or run: $bin_dir/mini-remote"
