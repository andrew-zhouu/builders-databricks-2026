#!/usr/bin/env bash
# Runs ON the remote machine: build the diagnostic binary (with the probes) in
# its own target directory and run it on a data set, as the diagnostic image
# would. The numbers are this machine's side of the comparison with the scorer.
# Usage: probe.sh [DATASET]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-dev}"
(cd fast && cargo build --release --features probe --target-dir "$PROJ/.toolchain/probe-target" > /tmp/ss-probe-build.log 2>&1) || { grep -E '^(error|warning: unused)' -A12 /tmp/ss-probe-build.log | head -40; exit 1; }
BIN="$PROJ/.toolchain/probe-target/x86_64-unknown-linux-musl/release/sessions"
out=/tmp/ss-probe; rm -rf "$out"; mkdir -p "$out"
NCPU=8 SS_DIAG=1 "$BIN" "data/$name" "$out" > "$out/stdout" 2> "$out/log"
echo "exit code: $? (1 is the intended result); log $(wc -c < "$out/log") bytes, $(wc -l < "$out/log") lines"
if [ "${ALL:-0}" = 1 ]; then cut -c1-200 "$out/log"; else sed -n '/== diag: probes/,/== diag: run 1/p; /== diag: summary/,$p' "$out/log" | cut -c1-220; fi
