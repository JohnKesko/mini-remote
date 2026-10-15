#!/bin/bash
# Stops and removes the fake Raspberry Pi test server container.
docker rm -f openvnc-testpi 2>/dev/null && echo "Stopped and removed openvnc-testpi." || echo "openvnc-testpi was not running."
