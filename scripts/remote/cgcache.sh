#!/usr/bin/env bash
# Runs ON the remote machine: simulated cache misses by source line (valgrind's
# cachegrind, one thread). "L1" is a 48 KB first-level cache; "LL" stands for
# what does not fit in LL_MB megabytes (default 2: roughly one thread's share
# of a second-level cache and a little of the third), i.e. the accesses that
# are slow. Counts are per event row.
# Usage: cgcache.sh [DATASET] [TOP] [FILE_REGEX]       env: LL_MB
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m5}"; top="${2:-45}"; filter="${3:-.}"
ll=$(( ${LL_MB:-2} * 1048576 ))
(cd fast && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --target-dir "$PROJ/.toolchain/prof-target" 2>&1 | grep -E '^error' -A8)
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
out=/tmp/ss-cgcache; rm -rf "$out"; mkdir -p "$out"
rows=$(cat "data/$name"/events/*.csv | wc -l)
NCPU=1 SS_NOHUGE=1 valgrind --tool=cachegrind --cache-sim=yes --D1=49152,12,64 --LL=$ll,16,64 --cachegrind-out-file="$out/cg.out" \
  "$PBIN" "data/$name" "$out/o" > "$out/log" 2>&1 || { tail -5 "$out/log"; exit 1; }
python3 -I - "$out/cg.out" "$rows" "$top" "$filter" <<'EOF'
import sys, re, collections
path, rows, top, filt = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
ir = collections.Counter(); l1 = collections.Counter(); ll = collections.Counter(); fn_ll = collections.Counter()
fl = fn = None; events = []
for line in open(path, errors="replace"):
    if line.startswith("events:"): events = line.split()[1:]; continue
    if line.startswith("fl="): fl = line[3:].strip(); continue
    if line.startswith("fn="): fn = line[3:].strip(); continue
    if not line[:1].isdigit(): continue
    p = line.split(); d = dict(zip(events, map(int, p[1:])))
    short = re.sub(r".*/fast/src/", "", fl or "?"); short = re.sub(r".*/library/", "lib:", short); short = re.sub(r".*/src_musl/", "musl:", short)
    k = (short, int(p[0]))
    ir[k] += d.get("Ir", 0); l1[k] += d.get("D1mr", 0) + d.get("D1mw", 0); m = d.get("DLmr", 0) + d.get("DLmw", 0); ll[k] += m
    fn_ll[re.sub(r"::h[0-9a-f]{16}$", "", fn or "?")[:70]] += m
print(f"per event row: {sum(ir.values())/rows:.0f} instructions, {sum(l1.values())/rows:.2f} L1 misses, {sum(ll.values())/rows:.2f} LL misses")
print("-- LL misses by function")
for f, v in fn_ll.most_common(12): print(f"  {v/rows:7.3f}  {f}")
print("-- LL misses by line (with L1 misses)")
n = 0
for k, v in ll.most_common():
    if not re.search(filt, k[0]): continue
    print(f"  {v/rows:7.3f}  L1 {l1[k]/rows:6.3f}  {k[0]}:{k[1]}"); n += 1
    if n >= top: break
print("-- L1 misses by line")
for k, v in l1.most_common(25): print(f"  {v/rows:7.3f}  {k[0]}:{k[1]}")
EOF
