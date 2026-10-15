#!/bin/bash
# Builds the image if needed and (re)starts the fake Raspberry Pi test server.
# Usage: ./test-server/start.sh [--rebuild]
set -e
cd "$(dirname "$0")"

NAME=openvnc-testpi
if [ "$1" = "--rebuild" ] || ! docker image inspect "$NAME" >/dev/null 2>&1; then
    docker build -t "$NAME" .
fi

docker rm -f "$NAME" >/dev/null 2>&1 || true

# All ports are published on 127.0.0.1 only. --init reaps stray child processes.
docker run -d --init --name "$NAME" \
    -p 127.0.0.1:5900:5900 \
    -p 127.0.0.1:5901:5901 \
    -p 127.0.0.1:5902:5902 \
    -p 127.0.0.1:2222:22 \
    "$NAME" >/dev/null

echo "Waiting for the desktop and the VNC servers to come up..."
for i in $(seq 1 60); do
    if python3 - <<'PY' 2>/dev/null
import socket, sys
for port in (5900, 5901, 5902):
    s = socket.create_connection(("127.0.0.1", port), timeout=2)
    if not s.recv(12).startswith(b"RFB"):
        sys.exit(1)
    s.close()
PY
    then
        break
    fi
    sleep 1
done

cat <<INFO

OpenVNC test server "$NAME" is running (listening on 127.0.0.1 only):
  127.0.0.1:5900  VNC, Raspberry Pi style: Linux user name + password (PAM), TLS / RSA-AES
  127.0.0.1:5901  VNC, classic password login (VNC authentication, TigerVNC + X11 desktop)
  127.0.0.1:5902  VNC, no login
  127.0.0.1:2222  SSH + SFTP

Test login (test values only):  user: pi   password: openvnc-test
(Port 5901 asks for a password only; classic VNC uses just the first 8 characters, "openvnc-".)
Check it:  python3 test-server/check.py      Stop it:  ./test-server/stop.sh
Logs:      docker logs $NAME
INFO
