#!/usr/bin/env bash
# Runs ON the remote machine: build the sources of another revision, exported
# beforehand into build/NAME of this tree's remote directory
# (`scripts/remote.sh export REV <remote dir>/build/NAME`), and keep the binary
# as bin/NAME. This tree's own sources and tools stay as they are.
# Usage: buildrev.sh NAME
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; dir="$PROJ/build/$name"
[ -d "$dir/fast" ] || { echo "no sources in $dir"; exit 2; }
out=$( (cd "$dir/fast" && cargo build --release 2>&1) )
if ! echo "$out" | grep -q 'Finished'; then echo "$out" | grep -E '^(error|warning: unused)' -A12 | head -60; exit 1; fi
echo "$out" | grep -E '^warning' -A6 | head -20
cp "$dir/fast/target/x86_64-unknown-linux-musl/release/sessions" "bin/$name" && echo "built bin/$name from $dir ($(wc -c < "bin/$name") bytes)"
grep -c -E 'avx512|zmm' /proc/cpuinfo | head -1 > /dev/null
echo "this machine: $(grep -m1 -o -E 'avx512vbmi2|avx512_vbmi2' /proc/cpuinfo || echo 'no avx512 vbmi2')"
