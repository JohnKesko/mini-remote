#!/bin/sh
# Usage: run-wayvnc.sh <config file> <port>. Restarts wayvnc if it ever stops.
# The port and address are set on the command line, the rest comes from the config file.
while true; do
    wayvnc -C "$1" -S "$XDG_RUNTIME_DIR/wayvncctl-$2" -L info 0.0.0.0 "$2"
    sleep 1
done
