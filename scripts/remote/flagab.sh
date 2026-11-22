#!/usr/bin/env bash
# Runs ON the remote machine (under the lock): build the pushed sources once
# more with extra compiler flags into a target directory of their own, keep
# that binary as bin/NAME, check it quickly, and time it against the ordinary
# build on both parser paths.
# Usage: flagab.sh NAME ROUNDS_20M ROUNDS_60M EXTRA_RUSTFLAGS...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; r20="$2"; r60="$3"; shift 3
base="-C target-cpu=skylake -C target-feature=+crt-static"
(cd fast && RUSTFLAGS="$base $*" cargo build --release --target-dir "$PROJ/out/flag-$name" 2>&1 | grep -E '^(error|warning: unused)' -A8 | head -20)
cp "out/flag-$name/x86_64-unknown-linux-musl/release/sessions" "bin/$name" || exit 1
ls -la "bin/$name" fast/target/x86_64-unknown-linux-musl/release/sessions | cut -c1-110
BIN="$PROJ/bin/$name" bash scripts/check.sh 2>&1 | tail -1
BIN="$PROJ/bin/$name" bash scripts/remote/scalecheck.sh m5 m20 | tail -3
bash scripts/abx.sh m20 "$r20" X=1 "SSBIN=$name" SS_SIMD=2 "SS_SIMD=2,SSBIN=$name" SS_SIMD=0 "SS_SIMD=0,SSBIN=$name" 2>&1 | tail -8
[ "$r60" -gt 0 ] && bash scripts/abx.sh m60 "$r60" SS_SIMD=2 "SS_SIMD=2,SSBIN=$name" SS_SIMD=0 "SS_SIMD=0,SSBIN=$name" 2>&1 | tail -6
