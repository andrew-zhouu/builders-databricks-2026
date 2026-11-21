#!/usr/bin/env bash
# Runs ON the remote machine: after cgasm.sh has profiled a build, show other
# functions from the same profile without running valgrind again.
# Usage: cgfn.sh FUNCTION_REGEX [MIN_PER_ROW] [HEAD_LINES] [DATASET]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
fn="$1"; min="${2:-0.02}"; head="${3:-60}"; name="${4:-dev}"
out=/tmp/ss-cgasm
rows=$(cat "data/$name"/events/*.csv | wc -l)
python3 -I "$PROJ/tools/cgasm.py" "$out/cg.out" "$out/asm.txt" "$rows" "$fn" "$min" | head -"$head" | cut -c1-130
# Every function's self cost, for the overall picture.
if [ "${TOTALS:-0}" = 1 ]; then
python3 -I - "$out/cg.out" "$out/asm.txt" "$rows" <<'EOF'
import sys, re, collections, bisect
cg, asm, rows = sys.argv[1], sys.argv[2], int(sys.argv[3])
count = collections.Counter(); skip = False
for line in open(cg, errors="replace"):
    if line.startswith("calls="): skip = True; continue
    if not line.startswith("0x"): continue
    if skip: skip = False; continue
    p = line.split()
    if len(p) >= 3: count[int(p[0], 16)] += int(p[2])
starts = []; names = []
for line in open(asm, errors="replace"):
    m = re.match(r"^([0-9a-f]+) <(.*)>:$", line)
    if m: starts.append(int(m.group(1), 16)); names.append(m.group(2))
per = collections.Counter()
for a, c in count.items():
    i = bisect.bisect_right(starts, a) - 1
    per[names[i] if i >= 0 else "?"] += c
tot = sum(per.values())
print(f"== all functions: {tot/rows:.1f} instructions per event row")
for n, c in per.most_common(40): print(f"  {c/rows:8.1f}  {n[:100]}")
EOF
fi
