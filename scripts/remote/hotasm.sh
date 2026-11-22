#!/usr/bin/env bash
# Runs ON the remote machine: disassembly of an address range of the profiled
# binary with the share of the program's samples on each instruction (from the
# last hotip.sh / lineprof.sh run).
# Usage: hotasm.sh START_HEX STOP_HEX [MIN_PERCENT_TO_MARK]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
PBIN="$PROJ/out/prof-target/x86_64-unknown-linux-musl/release/sessions"
objdump -d --no-show-raw-insn -C --start-address="0x$1" --stop-address="0x$2" "$PBIN" > /tmp/ss-hotasm.txt
python3 -I - "${3:-0.05}" <<'EOF'
import sys, re
mark = float(sys.argv[1])
m = re.search(r"\[(0x[0-9a-f]+)\((0x[0-9a-f]+)\) @ (0x[0-9a-f]+|0) ", open("/tmp/ss-hotip.map").read())
start, size, pgoff = int(m.group(1), 16), int(m.group(2), 16), int(m.group(3), 16)
off, vaddr = [int(x, 16) for x in open("/tmp/ss-hotip.seg").read().split()[:2]]
rows = [l.split() for l in open("/tmp/ss-hotip.txt")]
total = int(rows[0][2])
pct = {}
for n, ip, _ in rows:
    ip = int(ip, 16)
    if start <= ip < start + size:
        pct[ip - start + pgoff - off + vaddr] = 100 * int(n) / total
tot = 0
for line in open("/tmp/ss-hotasm.txt"):
    mm = re.match(r"\s*([0-9a-f]+):\s+(.*)", line)
    if not mm: continue
    a = int(mm.group(1), 16); p = pct.get(a, 0); tot += p
    print(f"{p:5.2f} {'*' if p >= mark else ' '} {a:x}: {mm.group(2).strip()[:110]}")
print(f"-- {tot:.2f}% of the program's samples in this range")
EOF
