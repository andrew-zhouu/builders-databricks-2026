#!/usr/bin/env bash
# Runs ON the remote machine: how much output is still not on disk at the
# moment a run ends? (That is what the wait after the container's exit is
# for; see sys::start_writeback.) Runs each binary on a data set a few times,
# writing to the root disk, and reads the kernel's dirty and under-writeback
# page counts as soon as the process has exited.
# Usage: dirtycheck.sh DATASET ROUNDS BINARY_NAME...     (names in bin/, or "cur" for the current build)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; rounds="$2"; shift 2
cat "data/$name"/*/*.csv > /dev/null
out="$PROJ/out/dirtycheck"
for r in $(seq "$rounds"); do
  for b in "$@"; do
    bin="$PROJ/bin/$b"; [ "$b" = cur ] && bin="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
    rm -rf "$out"; mkdir -p "$out"; sync; sleep 0.5
    before=$(awk '/^Dirty:|^Writeback:/ { s += $2 } END { print s }' /proc/meminfo)
    NCPU=8 "$bin" "data/$name" "$out" 2> /dev/null
    after=$(awk '/^Dirty:/ { d = $2 } /^Writeback:/ { w = $2 } END { print d + w, d, w }' /proc/meminfo)
    set -- "$@"
    echo "$b: before ${before} kB, at exit dirty+writeback $(echo "$after" | cut -d' ' -f1) kB (dirty $(echo "$after" | cut -d' ' -f2), writeback $(echo "$after" | cut -d' ' -f3)); output $(du -sk "$out" | cut -f1) kB"
  done
done
rm -rf "$out"
