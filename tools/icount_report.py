#!/usr/bin/env python3
"""
Turn an icount histogram into a profile by source function and source line.

Usage: icount_report.py HIST DISASM SRC_DIR [EVENT_LINES CLICK_LINES LINK_LINES]

DISASM is `llvm-objdump -d -l --no-show-raw-insn` of the same binary built with
line tables. Each instruction is attributed to the source line the line table
gives it (the innermost inlined location), and that line to the function whose
definition encloses it, so inlined helpers are reported under their own names.
"""
import collections
import os
import re
import sys


def main():
    hist_path, disasm_path, src_dir = sys.argv[1:4]
    lines_in = [int(x) for x in sys.argv[4:7]] if len(sys.argv) >= 7 else None

    counts = {}
    total = 0
    for line in open(hist_path):
        a, c = line.split()
        if a == "total":
            total = int(c)
        else:
            counts[int(a, 16)] = int(c)

    # address -> (symbol, file, line)
    where = {}
    symbol, loc = "?", ("?", 0)
    sym_re = re.compile(r"^[0-9a-f]+ <(.+)>:$")
    loc_re = re.compile(r"^; (.+):(\d+)$")
    ins_re = re.compile(r"^\s*([0-9a-f]+):\s")
    for line in open(disasm_path, errors="replace"):
        m = ins_re.match(line)
        if m:
            where[int(m.group(1), 16)] = (symbol, loc[0], loc[1])
            continue
        m = loc_re.match(line)
        if m:
            loc = (m.group(1), int(m.group(2)))
            continue
        m = sym_re.match(line)
        if m:
            symbol = m.group(1)

    # source line -> enclosing fn, for our own sources
    fn_re = re.compile(r"^\s*(?:pub(?:\([a-z]+\))?\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+([A-Za-z0-9_]+)")
    fns = {}
    for name in os.listdir(src_dir):
        if name.endswith(".rs"):
            starts = []
            for i, text in enumerate(open(os.path.join(src_dir, name)), 1):
                m = fn_re.match(text)
                if m:
                    starts.append((i, m.group(1)))
            fns[name] = starts

    def source_fn(path, line):
        base = os.path.basename(path)
        if "/fast/src/" not in path.replace("\\", "/") and not path.startswith("src/"):
            # std / core / compiler builtins: group by crate-ish path tail
            parts = path.replace("\\", "/").split("/")
            if "library" in parts:
                i = parts.index("library")
                return "std:" + "/".join(parts[i + 1:i + 3])
            return "other:" + base
        best = "?"
        for start, fname in fns.get(base, ()):
            if start <= line:
                best = fname
            else:
                break
        return f"{base[:-3]}::{best}"

    by_fn = collections.Counter()
    by_line = collections.Counter()
    by_sym = collections.Counter()
    unknown = 0
    for addr, c in counts.items():
        w = where.get(addr)
        if w is None:
            unknown += c
            continue
        sym, path, line = w
        by_sym[demangle(sym)] += c
        f = source_fn(path, line)
        by_fn[f] += c
        by_line[(f, os.path.basename(path), line)] += c

    print(f"total instructions: {total:,}   (not in disassembly: {unknown:,})")
    if lines_in:
        ev, cl, ln = lines_in
        print(f"input lines: events {ev:,}  clicks {cl:,}  links {ln:,}")
    print("\n-- by top-level symbol")
    for s, c in by_sym.most_common(14):
        print(f"  {c:12,} {100 * c / total:5.1f}%  {s}")
    print("\n-- by source function (inlined code under its own name)")
    for f, c in by_fn.most_common(40):
        per = ""
        if lines_in and lines_in[0]:
            per = f"  {c / lines_in[0]:7.1f} /event line"
        print(f"  {c:12,} {100 * c / total:5.1f}%  {f:34s}{per}")
    print("\n-- hottest source lines")
    for (f, base, line), c in by_line.most_common(int(os.environ.get("TOP_LINES", "45"))):
        print(f"  {c:12,} {100 * c / total:5.1f}%  {base}:{line:<5d} {f}")
    # With SRC_LINES=file.rs: that file's lines in order, each with its share.
    want = os.environ.get("SRC_LINES")
    if want:
        per_line = collections.Counter()
        for (f, base, line), c in by_line.items():
            if base == want:
                per_line[line] += c
        print(f"\n-- {want}, line by line (samples; share of all)")
        text = open(os.path.join(src_dir, want)).read().split("\n")
        for line in sorted(per_line):
            c = per_line[line]
            if c * 500 >= total:
                print(f"  {c:8,} {100 * c / total:5.1f}%  {line:5d}: {text[line - 1].strip()[:110]}")


def demangle(sym):
    # v0 mangling: keep the readable path components
    parts = re.findall(r"\d+([A-Za-z_][A-Za-z0-9_]*)", sym)
    if sym.startswith("_R") and parts:
        out = []
        for p in parts:
            out.append(p)
        return "::".join(out[-3:])
    return sym


if __name__ == "__main__":
    main()
