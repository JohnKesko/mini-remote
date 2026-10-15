#!/bin/bash
# Moving colour bars: many pixels change every frame (good for testing the encoder).
printf '\033[?25l\033[2J'
t=0
while :; do
    printf '\033[H'
    for row in $(seq 0 15); do
        for col in $(seq 0 79); do
            printf '\033[48;5;%dm ' $(( 16 + (col + row * 2 + t) % 216 ))
        done
        printf '\033[0m\n'
    done
    t=$((t + 3))
    sleep 0.1
done
