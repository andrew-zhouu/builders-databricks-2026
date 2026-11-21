#!/usr/bin/env bash
# Runs ON the remote machine: exact instruction counts (and simulated branch
# mispredictions) by source line, from valgrind, for a single-threaded run of
# the current sources on a small data set. The machine has no hardware
# counters, so this is how instruction diets are measured there.
# Usage: cg.sh [DATASET] [TOP] [FILE_REGEX]      env: BRANCH=1 for mispredictions
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-dev}"; top="${2:-70}"; filter="${3:-.}"
command -v valgrind >/dev/null || sudo apt-get install -y -qq valgrind >/dev/null 2>&1
(cd fast && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release --target-dir "$PROJ/.toolchain/prof-target" 2>&1 | grep -E '^error' -A8)
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
data="data/$name"; [ -d "$data" ] || data="official/dev-answer/$name"
out=/tmp/ss-cg; rm -rf "$out"; mkdir -p "$out"
rows=$(cat "$data"/events/*.csv | wc -l)
sim=""; [ "${BRANCH:-0}" = 1 ] && sim="--branch-sim=yes"
NCPU=1 SS_NOHUGE=1 valgrind --tool=cachegrind --cache-sim=no $sim --cachegrind-out-file="$out/cg.out" \
  "$PBIN" "$data" "$out/o" > "$out/log" 2>&1 || { tail -5 "$out/log"; exit 1; }
echo "event rows: $rows"
cg_annotate --show-percs=no --threshold=0.01 "$out/cg.out" 2>/dev/null > "$out/ann.txt" || cg_annotate "$out/cg.out" > "$out/ann.txt"
python3 -I - "$out/cg.out" "$rows" "$top" "$filter" "${BRANCH:-0}" <<'EOF'
import sys, re, collections
path, rows, top, filt, branch = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5] == "1"
per = collections.Counter(); bm = collections.Counter(); perfile = collections.Counter()
fl = None; events = []
for line in open(path, errors="replace"):
    if line.startswith("events:"): events = line.split()[1:]; continue
    if line.startswith("fl="): fl = line[3:].strip(); continue
    if line.startswith(("fn=", "desc:", "cmd:", "summary:", "totals:")) or not line[:1].isdigit(): continue
    p = line.split()
    ln = int(p[0]); vals = list(map(int, p[1:]))
    d = dict(zip(events, vals))
    short = re.sub(r".*/fast/src/", "", fl or "?"); short = re.sub(r".*/library/", "lib:", short)
    per[(short, ln)] += d.get("Ir", 0); perfile[short] += d.get("Ir", 0)
    bm[(short, ln)] += d.get("Bcm", 0) + d.get("Bim", 0)
total = sum(per.values())
print(f"total Ir {total/1e6:.1f} M = {total/rows:.1f} per event row")
print("-- by file (Ir per event row)")
for f, v in perfile.most_common(14): print(f"  {v/rows:8.1f}  {f}")
print("-- by line")
n = 0
for (f, ln), v in per.most_common():
    if not re.search(filt, f): continue
    extra = f"  mispred/row {bm[(f, ln)]/rows:.3f}" if branch else ""
    print(f"  {v/rows:8.2f}  {f}:{ln}{extra}")
    n += 1
    if n >= top: break
if branch:
    print(f"-- mispredictions: {sum(bm.values())/rows:.3f} per event row; top lines")
    for (f, ln), v in bm.most_common(25): print(f"  {v/rows:8.3f}  {f}:{ln}")
EOF
