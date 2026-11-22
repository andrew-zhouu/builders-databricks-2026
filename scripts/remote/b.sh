#!/usr/bin/env bash
# Runs ON the remote machine: build the pushed sources and show what the
# compiler says (errors and warnings, shortened), then the binary.
# Usage: b.sh [LINES]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ/fast"
cargo build --release 2>&1 | grep -E -A16 '^(error|warning)' | grep -v -e '^warning: .* generated [0-9]* warning' | head -"${1:-80}" | cut -c1-200
ls -la target/x86_64-unknown-linux-musl/release/sessions | cut -c1-70
