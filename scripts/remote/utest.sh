#!/usr/bin/env bash
# Runs ON the remote machine: the unit tests of the pushed sources.
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ/fast"
cargo test --release 2>&1 | grep -E '^test result|FAILED|panicked|^error' | cut -c1-160
