#!/usr/bin/env bash
# Build, sign and publish a new Mini Remote release on GitHub. Installed copies
# find it on their next start and offer to update.
#
#   1. Raise the version in the root Cargo.toml (for example 0.2.0), commit and push.
#   2. scripts/release.sh [--notes FILE] [--draft]
#
# Builds:   Mac (Apple Silicon, macOS 12+) and Linux (x86_64 and arm64, in Docker).
# Signs:    every file with the key in ~/.config/mini-remote/release-signing-key
#           (made once with: cargo run -p mini-remote-update --bin mini-remote-release -- keygen).
# Publishes: GitHub release vX.Y.Z with the files and their .sig signatures.
#           Without --notes, GitHub writes the notes from the commits since the last release.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

notes=""
draft=""
while [ $# -gt 0 ]; do
    case "$1" in
        --notes) notes="$2"; shift 2 ;;
        --draft) draft="--draft"; shift ;;
        *) echo "usage: scripts/release.sh [--notes FILE] [--draft]" >&2; exit 2 ;;
    esac
done

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
tag="v$version"
key="$HOME/.config/mini-remote/release-signing-key"
out="dist/release/$version"

fail() { echo "error: $*" >&2; exit 1; }

# Checks before building anything.
[ -f "$key" ] || fail "no signing key at $key"
gh auth status >/dev/null 2>&1 || fail "not logged in to GitHub (run: gh auth login)"
[ -z "$(git status --porcelain)" ] || fail "there are uncommitted changes; commit them first"
git fetch -q origin
[ "$(git rev-parse HEAD)" = "$(git rev-parse '@{u}')" ] || fail "push your commits first (the release is made from GitHub's copy)"
if gh release view "$tag" >/dev/null 2>&1; then
    fail "release $tag already exists; raise the version in Cargo.toml"
fi

echo "== Building Mini Remote $version"
rm -rf "$out"
mkdir -p "$out"

scripts/macos/bundle.sh
ditto -c -k --keepParent "dist/Mini Remote.app" "$out/Mini-Remote-$version-macos-arm64.zip"

scripts/linux/build.sh all
cp "dist/linux/mini-remote-$version-linux-x86_64.tar.gz" "dist/linux/mini-remote-$version-linux-arm64.tar.gz" "$out/"

echo "== Signing"
release_tool=(cargo run -q --release -p mini-remote-update --bin mini-remote-release --)
files=("$out"/*.zip "$out"/*.tar.gz)
"${release_tool[@]}" sign --version "$version" --key "$key" "${files[@]}"
# Check with the public key built into the app, so a wrong key is caught here.
"${release_tool[@]}" verify --version "$version" "${files[@]}"

echo "== Publishing $tag"
notes_args=(--generate-notes)
[ -n "$notes" ] && notes_args=(--notes-file "$notes")
gh release create "$tag" "$out"/* --title "Mini Remote $version" --target "$(git rev-parse HEAD)" "${notes_args[@]}" $draft

echo "Released $tag:"
ls -lh "$out"
