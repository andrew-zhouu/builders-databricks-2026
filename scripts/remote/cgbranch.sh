#!/usr/bin/env bash
# Runs ON the remote machine: simulated branch mispredictions by source line
# and by function (valgrind's cachegrind, one thread), per event row. A
# misprediction costs 15 to 20 cycles; an event line costs about 330 in all.
# Usage: cgbranch.sh [DATASET] [TOP] [FILE_REGEX]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m5}"; top="${2:-40}"; filter="${3:-.}"
(cd fast && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --target-dir "$PROJ/out/prof-target" 2>&1 | grep -E '^error' -A8)
PBIN="$PROJ/out/prof-target/x86_64-unknown-linux-musl/release/sessions"
out=/tmp/ss-cgbranch; rm -rf "$out"; mkdir -p "$out"
rows=$(cat "data/$name"/events/*.csv | wc -l)
NCPU=1 SS_NOHUGE=1 valgrind --tool=cachegrind --cache-sim=no --branch-sim=yes --cachegrind-out-file="$out/cg.out" \
  "$PBIN" "data/$name" "$out/o" > "$out/log" 2>&1 || { tail -5 "$out/log"; exit 1; }
python3 -I - "$out/cg.out" "$rows" "$top" "$filter" <<'EOF'
import sys, re, collections
path, rows, top, filt = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4]
ir = collections.Counter(); bc = collections.Counter(); bm = collections.Counter(); fn_bm = collections.Counter(); fn_ir = collections.Counter()
fl = fn = None; events = []
for line in open(path, errors="replace"):
    if line.startswith("events:"): events = line.split()[1:]; continue
    if line.startswith("fl="): fl = line[3:].strip(); continue
    if line.startswith("fn="): fn = line[3:].strip(); continue
    if not line[:1].isdigit(): continue
    p = line.split(); d = dict(zip(events, map(int, p[1:])))
    short = re.sub(r".*/fast/src/", "", fl or "?"); short = re.sub(r".*/library/", "lib:", short); short = re.sub(r".*/src_musl/", "musl:", short)
    k = (short, int(p[0])); m = d.get("Bcm", 0) + d.get("Bim", 0)
    ir[k] += d.get("Ir", 0); bc[k] += d.get("Bc", 0) + d.get("Bi", 0); bm[k] += m
    f = re.sub(r"::h[0-9a-f]{16}$", "", fn or "?")[:70]; fn_bm[f] += m; fn_ir[f] += d.get("Ir", 0)
print(f"per event row: {sum(ir.values())/rows:.0f} instructions, {sum(bc.values())/rows:.1f} branches, {sum(bm.values())/rows:.2f} mispredicted")
print("-- mispredictions by function (and instructions)")
for f, v in fn_bm.most_common(14): print(f"  {v/rows:7.3f}  {fn_ir[f]/rows:7.1f}  {f}")
print("-- mispredictions by line (branches there)")
n = 0
for k, v in bm.most_common():
    if not re.search(filt, k[0]): continue
    print(f"  {v/rows:7.3f}  of {bc[k]/rows:6.2f}  {k[0]}:{k[1]}"); n += 1
    if n >= top: break
EOF
