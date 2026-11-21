#!/usr/bin/env bash
# Build the fast pipeline (static musl binary) -> fast/target/.../release/sessions
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ/fast"
cargo build --release 2>&1 | grep -v '^\s*Compiling\|^\s*Finished' || true
ls -la "$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions" | awk '{print "binary:", $5, "bytes"}'
