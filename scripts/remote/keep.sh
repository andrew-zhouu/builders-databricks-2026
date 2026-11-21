#!/usr/bin/env bash
# Runs ON the remote machine: build the pushed sources, check them quickly
# (fixtures, dev answer, a few fuzz seeds) and keep the binary as bin/NAME, for
# A/B runs between variants (abx.sh ... SSBIN=NAME).
# Usage: keep.sh NAME [FUZZ_COUNT]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"
bash scripts/remote/try.sh "${2:-6}" 3000 2>&1 | tail -3 | cut -c1-110
cp fast/target/x86_64-unknown-linux-musl/release/sessions "bin/$name" && echo "kept bin/$name"
