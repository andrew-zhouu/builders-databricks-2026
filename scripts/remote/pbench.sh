#!/usr/bin/env bash
# Runs ON the remote machine: the parsers alone on one thread (main.rs bench()).
# A small region repeated many times stays in the cache (pure compute); a large
# one shows what memory adds.
# Usage: pbench.sh [DATASET]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"
B="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
echo "-- 512 KB region x 400 (in cache), one thread on cpu 2"
for _ in 1 2; do NCPU=1 taskset -c 2 "$B" --bench "data/$name" /tmp/ss-b 2>&1 | grep '^bench' | cut -c1-150; done
echo "-- 64 MB region x 6 (from memory), one thread on cpu 2"
SS_BENCH_BYTES=67108864 SS_BENCH_ITERS=6 NCPU=1 taskset -c 2 "$B" --bench "data/$name" /tmp/ss-b 2>&1 | grep '^bench' | cut -c1-150
echo "-- 64 MB region, two copies at once on sibling threads (cpus 2 and 6)"
(SS_BENCH_BYTES=67108864 SS_BENCH_ITERS=6 NCPU=1 taskset -c 6 "$B" --bench "data/$name" /tmp/ss-b2 > /dev/null 2>&1 &)
SS_BENCH_BYTES=67108864 SS_BENCH_ITERS=6 NCPU=1 taskset -c 2 "$B" --bench "data/$name" /tmp/ss-b 2>&1 | grep '^bench' | cut -c1-150
wait
