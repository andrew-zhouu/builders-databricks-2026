#!/usr/bin/env bash
# Runs ON the remote machine: build and show cargo's own last lines (when
# try.sh says nothing useful).
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
echo "PROJ=$PROJ"; which cargo rustc; ls -la .toolchain | head -3
(cd fast && cargo build --release 2>&1 | tail -25)
ls -la fast/target/x86_64-unknown-linux-musl/release/sessions 2>&1 | tail -1
