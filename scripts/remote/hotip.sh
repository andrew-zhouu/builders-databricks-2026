#!/usr/bin/env bash
# Runs ON the remote machine: the hottest instructions of the last lineprof.sh
# run (REUSE=1) or of a new one, each with the chain of inlined source lines it
# belongs to. Shows what a "file.rs:0" entry of the line profile really is.
# Usage: hotip.sh [DATASET] [SOURCE_REGEX] [TOP]        env: REUSE=1
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"; re="${2:-.}"; top="${3:-40}"
PBIN="$PROJ/.toolchain/prof-target/x86_64-unknown-linux-musl/release/sessions"
if [ "${REUSE:-0}" != 1 ]; then bash scripts/remote/lineprof.sh "$name" 5 . > /dev/null 2>&1; fi
perf script -i /tmp/ss-line.data -F comm,ip 2>/dev/null | awk '$1 == "sessions" { n[$2]++; t++ } END { for (a in n) printf "%d %s %d\n", n[a], a, t }' \
  | sort -rn | head -3000 > /tmp/ss-hotip.txt
# Where the program's code was mapped (it is position-independent).
perf script -i /tmp/ss-line.data --show-mmap-events 2>/dev/null | grep -m1 -E 'r-xp .*release/sessions' > /tmp/ss-hotip.map
readelf -lW "$PBIN" | awk '$1 == "LOAD" && $0 ~ /R E/ { print $2, $3 }' > /tmp/ss-hotip.seg
python3 -I - "$PBIN" "$re" "$top" <<'EOF'
import sys, subprocess, re, collections
pbin, rx, top = sys.argv[1], re.compile(sys.argv[2]), int(sys.argv[3])
m = re.search(r"\[(0x[0-9a-f]+)\((0x[0-9a-f]+)\) @ (0x[0-9a-f]+|0) ", open("/tmp/ss-hotip.map").read())
start, size, pgoff = int(m.group(1), 16), int(m.group(2), 16), int(m.group(3), 16)
off, vaddr = [int(x, 16) for x in open("/tmp/ss-hotip.seg").read().split()[:2]]
rows = [l.split() for l in open("/tmp/ss-hotip.txt")]
total = int(rows[0][2]) if rows else 1
mine = [(int(r[0]), int(r[1], 16)) for r in rows if start <= int(r[1], 16) < start + size]
addrs = [ip - start + pgoff - off + vaddr for _, ip in mine]
# One addr2line process: -a prints the address before each chain of frames.
out = subprocess.run(["addr2line", "-a", "-i", "-e", pbin] + [hex(a) for a in addrs], capture_output=True, text=True).stdout
chains, cur = [], None
for line in out.split("\n"):
    if line.startswith("0x"):
        cur = []; chains.append(cur)
    elif line and cur is not None:
        line = re.sub(r".*/fast/src/", "", line); line = re.sub(r".*/library/", "lib:", line)
        cur.append(re.sub(r" \(discriminator \d+\)", "", line))
by_chain = collections.Counter(); shown = 0
for (n, ip), a, chain in zip(mine, addrs, chains):
    text = " < ".join(chain[:6])
    if not rx.search(text): continue
    by_chain[text] += n
    if shown < top:
        print(f"{100*n/total:6.2f}%  {a:x}  {text[:150]}"); shown += 1
print("-- by inline chain")
for text, n in by_chain.most_common(top):
    print(f"{100*n/total:6.2f}%  {text[:160]}")
print(f"-- {100*sum(by_chain.values())/total:.2f}% of the program's samples match")
EOF
