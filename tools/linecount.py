#!/usr/bin/env python3
"""
Static instruction counts per source line inside one function of the profiling
disassembly (llvm-objdump -d -l). For straight-line code this approximates the
dynamic count per call. Usage: linecount.py DISASM SYMBOL_SUBSTRING [TOP]
"""
import collections
import re
import sys

disasm, needle = sys.argv[1], sys.argv[2]
top = int(sys.argv[3]) if len(sys.argv) > 3 else 60
inside = False
loc = "?"
counts = collections.Counter()
first = {}
total = 0
for line in open(disasm, errors="replace"):
    m = re.match(r"^[0-9a-f]+ <(.+)>:$", line)
    if m:
        inside = needle in m.group(1)
        continue
    if not inside:
        continue
    m = re.match(r"^; (.+):(\d+)$", line)
    if m:
        path = m.group(1).replace("\\", "/")
        loc = path.split("/")[-1] + ":" + m.group(2)
        continue
    m = re.match(r"^\s*([0-9a-f]+):\s+(\S+)", line)
    if m:
        counts[loc] += 1
        first.setdefault(loc, m.group(1))
        total += 1
print(f"{total} instructions in functions matching {needle!r}")
for loc, c in counts.most_common(top):
    print(f"  {c:5d}  {loc:28s} first at {first[loc]}")
