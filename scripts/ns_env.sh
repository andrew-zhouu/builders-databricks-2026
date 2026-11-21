#!/usr/bin/env bash
# One-off (dev line): give every experiment/test switch the binary reads from
# the environment an SS_ prefix, so that no unrelated variable in a scorer's
# environment (TIMING, IO, SLOW, ...) can ever change how a run behaves.
# DATA_DIR, OUTPUT_DIR and NCPU are the competition's own names and stay.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
names="TIMING SLOW NOHUGE NORELEASE BENCH_BYTES BENCH_ITERS SCALAR PREFETCH NOZAP EAGER_LOOKUPS CHUNK_BYTES IO READ_TAIL"
for n in $names; do
  sed -i "s/var(\"$n\")/var(\"SS_$n\")/g" fast/src/main.rs
done
for f in scripts/*.sh tools/*.sh; do
  [ "$f" = scripts/ns_env.sh ] && continue
  sed -i -E \
    -e 's/(^|[^A-Za-z_{])(TIMING|SCALAR|NOZAP|NOHUGE|NORELEASE|EAGER_LOOKUPS)=1/\1SS_\2=1/g' \
    -e 's/(^|[^A-Za-z_{])(CHUNK_BYTES|READ_TAIL|PREFETCH|BENCH_BYTES|BENCH_ITERS)=/\1SS_\2=/g' \
    -e 's/(^|[^A-Za-z_{])IO=read/\1SS_IO=read/g' \
    "$f"
done
sed -i 's/`TIMING=1`/`SS_TIMING=1`/' README.md
echo "--- binary reads:"; grep -o 'var("[A-Z_]*")' fast/src/main.rs | sort | uniq -c
echo "--- scripts set:"; grep -ohE 'SS_[A-Z_]+=' scripts/*.sh tools/*.sh | sort | uniq -c
echo "--- unprefixed leftovers (should be script-level knobs only):"
grep -nE '(^|[^A-Za-z_{S])(TIMING|SCALAR|NOZAP|NOHUGE|NORELEASE|EAGER_LOOKUPS|CHUNK_BYTES|READ_TAIL|PREFETCH)=' scripts/*.sh tools/*.sh | grep -v ns_env || echo none
