#!/usr/bin/env bash
# Runs ON the remote machine: a Docker image of the pushed working tree (not
# of a committed revision), for experiments: only the files the Dockerfile
# reads are copied into the build context.
# Usage: wipimg.sh [TAG_SUFFIX]       -> shopping-sessions:wip (or :SUFFIX)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
tag="shopping-sessions:${1:-wip}"
ctx=/tmp/ss-wipimg; rm -rf "$ctx"; mkdir -p "$ctx/fast"
cp Dockerfile "$ctx/"; [ -f .dockerignore ] && cp .dockerignore "$ctx/"
cp -r fast/Cargo.toml fast/Cargo.lock fast/.cargo fast/src "$ctx/fast/"
(cd "$ctx" && docker build -q -t "$tag" . > /tmp/ss-wipimg.log 2>&1) || { echo "build failed"; tail -20 /tmp/ss-wipimg.log; exit 1; }
echo "built $tag"
