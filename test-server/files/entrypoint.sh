#!/bin/sh
# Container start: run the SSH server, then the headless labwc desktop as user "pi".
# The desktop's autostart (see labwc-autostart) launches the apps and the three wayvnc servers.
set -e

mkdir -p /run/sshd
ssh-keygen -A >/dev/null
/usr/sbin/sshd

install -d -o pi -g pi -m 700 /run/user/1000

# Second desktop for the classic VNC password login (port 5901), restarted if it stops.
(while true; do
    runuser -u pi -- /opt/testpi/xvnc-session.sh
    sleep 2
done) &

# Headless wlroots backend + software rendering, no input devices (wayvnc injects input).
exec runuser -u pi -- env \
    HOME=/home/pi USER=pi LOGNAME=pi SHELL=/bin/bash LANG=C.UTF-8 \
    XDG_RUNTIME_DIR=/run/user/1000 \
    XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=labwc:wlroots \
    WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
    WLR_NO_HARDWARE_CURSORS=1 \
    dbus-run-session -- labwc
