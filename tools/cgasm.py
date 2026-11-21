#!/usr/bin/env python3
"""Join a callgrind profile (--dump-instr=yes --compress-pos=no
--compress-strings=no) with `objdump -d -l` and print the instructions of the
functions matching a regex that ran at least MIN times per row, in address
order, with their per-row counts and source lines.

usage: cgasm.py CALLGRIND_OUT OBJDUMP_TXT ROWS FUNCTION_REGEX [MIN_PER_ROW]
"""
import collections
import re
import sys

cg, asm, rows, fn_re = sys.argv[1], sys.argv[2], int(sys.argv[3]), re.compile(sys.argv[4])
minimum = float(sys.argv[5]) if len(sys.argv) > 5 else 0.02

count = collections.Counter()
skip = False
for line in open(cg, errors="replace"):
    if line.startswith("calls="):
        skip = True          # the next cost line is the callee's inclusive cost
        continue
    if not line.startswith("0x"):
        continue
    if skip:
        skip = False
        continue
    p = line.split()
    if len(p) >= 3:
        count[int(p[0], 16)] += int(p[2])

func = None
src = ""
shown = 0
total = 0.0
by_src = collections.Counter()
out = []
for line in open(asm, errors="replace"):
    m = re.match(r"^[0-9a-f]+ <(.*)>:$", line)
    if m:
        func = m.group(1)
        continue
    if func is None or not fn_re.search(func):
        continue
    m = re.match(r"^(/\S+):(\d+)", line)
    if m:
        path = re.sub(r".*/fast/src/", "", m.group(1))
        path = re.sub(r".*/library/", "lib:", path)
        path = re.sub(r".*/src_musl/", "musl:", path)
        src = f"{path}:{m.group(2)}"
        continue
    m = re.match(r"^\s*([0-9a-f]+):\s+(.*)$", line)
    if not m:
        continue
    addr = int(m.group(1), 16)
    c = count.get(addr, 0) / rows
    if c >= minimum:
        total += c
        by_src[src] += c
        out.append(f"{c:7.3f}  {addr:x}  {m.group(2).strip()[:60]:60s} {src}")

print(f"== {fn_re.pattern}: {total:.1f} instructions per row on the shown path ({len(out)} instructions)")
print("-- by source line")
for s, c in by_src.most_common(45):
    print(f"  {c:7.2f}  {s}")
print("-- instructions")
print("\n".join(out))
