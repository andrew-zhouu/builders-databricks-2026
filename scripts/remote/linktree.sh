#!/usr/bin/env bash
# Runs ON the remote machine, in a secondary working tree: (re)create the links
# to what it shares with the main tree (data sets, reference answers,
# toolchain, Python environment, official material).
main="${1:-/home/ubuntu/db}"
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
for d in data expected .toolchain .venv official; do
  if [ -L "$d" ] || [ ! -e "$d" ]; then ln -sfn "$main/$d" "$d"; fi
done
mkdir -p out bin
ls -la | grep -E ' -> ' | cut -c40-
