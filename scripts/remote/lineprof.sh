#!/usr/bin/env bash
# Runs ON the remote machine (needs sudo): time by source line for one run of
# the current sources (rebuilt with line tables), sampled on every CPU.
# Usage: lineprof.sh [DATASET] [TOP] [FILTER_REGEX]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"; top="${2:-60}"; filter="${3:-.}"
(cd fast && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --target-dir "$PROJ/.toolchain/prof-target" 2>&1 | grep -E '^error' -A8)
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
out=/tmp/ss-lineprof; sudo rm -rf "$out"; mkdir -p "$out"
cat data/"$name"/*/*.csv > /dev/null
sudo perf record -q -F 2999 -a -o /tmp/ss-line.data -- env NCPU=8 ${RUN_ENV:-} "$PBIN" "data/$name" "$out" 2>/dev/null
sudo chown "$USER" /tmp/ss-line.data
perf report -i /tmp/ss-line.data --stdio --no-children --sort srcline ${SYM:+--symbol-filter=$SYM} 2>/dev/null \
  | grep -v '^#' | grep -v '^$' | sed 's#/home/[a-z]*/db/fast/src/##; s#/rustc/[0-9a-f]*/library/##' \
  | grep -E "$filter" | head -"$top" | cut -c1-110
sudo rm -rf "$out"
