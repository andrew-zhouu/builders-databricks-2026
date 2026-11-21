#!/usr/bin/env bash
# Which candidate does origin/main build? Compares main's tree with each
# candidate commit: everything, and only what the Docker build reads.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
GCM_INTERACTIVE=Never GIT_TERMINAL_PROMPT=0 git fetch -q origin 2>/dev/null || true
echo "origin/main = $(git rev-parse origin/main)"
for r in "$@"; do
  all=$(git diff --stat "$r" origin/main | tail -1 | tr -s ' ')
  build=$(git diff --stat "$r" origin/main -- fast Dockerfile .dockerignore | tail -1 | tr -s ' ')
  echo "$r vs main: all [${all:- identical}]  build inputs [${build:- identical}]"
done
