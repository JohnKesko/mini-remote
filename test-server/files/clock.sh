#!/bin/sh
# Prints the date, time and memory use once a second (something that changes on screen).
while true; do
    printf '\033[H\033[J'
    date '+%A %d %B %Y'
    date '+%H:%M:%S'
    echo
    uptime -p
    free -m | awk 'NR==2 {print "Memory used: " $3 " of " $2 " MB"}'
    sleep 1
done
