#!/bin/sh
# Classic VNC password desktop on port 5901 (security type 2, "VNC authentication").
# wayvnc cannot do this, so this is TigerVNC's Xvnc with a light X11 desktop.
# Note: the VNC password is only 8 characters long on the wire, so "openvnc-test" counts as "openvnc-".
export HOME=/home/pi DISPLAY=:1 LANG=C.UTF-8

Xvnc :1 -geometry 1920x1080 -depth 24 -rfbport 5901 -AlwaysShared -ac \
    -SecurityTypes VncAuth -PasswordFile /etc/testpi/vncpasswd \
    -desktop "openvnc-testpi (VNC password)" &
XVNC_PID=$!

for i in $(seq 1 50); do [ -S /tmp/.X11-unix/X1 ] && break; sleep 0.2; done

feh --bg-fill /usr/share/testpi/wallpaper.png &
openbox &
xterm -T "top (live)" -geometry 100x28+40+40 -fa 'DejaVu Sans Mono' -fs 12 -e top -d 1 &
xterm -T "clock" -geometry 60x9+1000+40 -fa 'DejaVu Sans Mono' -fs 12 -e /opt/testpi/clock.sh &
xterm -T "moving colours" -geometry 80x16+1000+420 -fa 'DejaVu Sans Mono' -fs 8 -e /opt/testpi/rainbow.sh &
GDK_BACKEND=x11 dbus-run-session -- mousepad /home/pi/Documents/notes.txt &

wait $XVNC_PID
