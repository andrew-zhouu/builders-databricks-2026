#!/usr/bin/env bash
# Digest of a battery's output (idxfuzz.sh / fuzzidx.sh lines): per failing set
# the settings that failed, split into those with SS_SIMD=1 (the wide parser,
# off by default on exp-v5) and the others, with the first message of each
# kind; then the summary lines.
# Usage: failsum.sh OUTPUT_FILE
file="$1"
python3 -I - "$file" <<'EOF'
import re, sys
sets = {}
order = []
for line in open(sys.argv[1], errors="replace"):
    m = re.match(r"FAIL seed (\d+)(.*?)(?: setting)? ([\w-]+) \(([^)]*)\): (.*)", line)
    if not m:
        continue
    seed, what, label, settings, message = m.groups()
    key = (seed, what.strip())
    if key not in sets:
        sets[key] = {"simd": [], "other": [], "first": {}}
        order.append(key)
    kind = "simd" if "SS_SIMD=1" in settings else "other"
    sets[key][kind].append(label)
    sets[key]["first"].setdefault(kind, f"{label} ({settings}): {message.strip()[:420]}")
for key in order:
    s = sets[key]
    print(f"seed {key[0]} {key[1]}: default-path settings failing: {len(s['other'])} {s['other'][:12]}; SS_SIMD=1 settings failing: {len(s['simd'])}")
    for kind in ("other", "simd"):
        if kind in s["first"]:
            print(f"    first {kind}: {s['first'][kind]}")
EOF
grep -E '^(idxfuzz|fuzzidx|TOTAL|wixscale|V5CHECK|SELFTEST)' "$file" | cut -c1-200
