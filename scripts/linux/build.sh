#!/usr/bin/env bash
# Builds the Linux release in Docker and writes
# dist/linux/mini-remote-<version>-linux-<arch>.tar.gz
#
# Usage: scripts/linux/build.sh [x86_64|arm64|all]    (default: all)
# RUST_VERSION picks the Rust release used inside the build (default below).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version *= *"\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
rust_version=${RUST_VERSION:-1.99.0}

case "${1:-all}" in
    x86_64 | amd64) arches=(x86_64) ;;
    arm64 | aarch64) arches=(arm64) ;;
    all) arches=(x86_64 arm64) ;;
    *)
        echo "Usage: $0 [x86_64|arm64|all]" >&2
        exit 2
        ;;
esac

if [ -z "$version" ]; then
    echo "Could not read the version from Cargo.toml" >&2
    exit 1
fi

mkdir -p "$root/dist/linux"
for arch in "${arches[@]}"; do
    case $arch in
        x86_64) platform=linux/amd64 ;;
        arm64) platform=linux/arm64 ;;
    esac
    echo "Building Mini Remote $version for Linux $arch..."
    DOCKER_BUILDKIT=1 docker build \
        --platform "$platform" \
        --file "$root/scripts/linux/Dockerfile" \
        --build-arg RUST_VERSION="$rust_version" \
        --build-arg VERSION="$version" \
        --build-arg ARCH="$arch" \
        --output "type=local,dest=$root/dist/linux" \
        "$root"
    archive="$root/dist/linux/mini-remote-$version-linux-$arch.tar.gz"
    echo "Made $archive ($(du -h "$archive" | cut -f1))"
done
