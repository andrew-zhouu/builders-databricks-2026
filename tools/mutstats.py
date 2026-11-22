#!/usr/bin/env python3
"""
What shapes does a data set hold? Counts of the things the mutations of
tools/mutate.py are meant to produce, to see that they are really there:
line lengths around 64 and 128 bytes, fields quoted per line, lengths of every
field, ts forms, events on the 24-hour edge (and those whose ts shows the
date of their ingest_ts while being more than 24 hours before it), repeated
event IDs, files without a final newline.

Usage: mutstats.py DATA_DIR
"""
import collections
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mutate import DAY, parse_ts, split_fields  # noqa: E402


def bucket(n, edges=(0, 1, 2, 8, 9, 10, 11, 16, 17, 24, 32, 33, 64, 65)):
    for e in edges:
        if n <= e:
            return f"<={e}"
    return f">{edges[-1]}"


def main():
    data = sys.argv[1]
    lengths = collections.Counter()
    quoted = collections.Counter()
    forms = collections.Counter()
    field_len = [collections.Counter() for _ in range(7)]
    edge = collections.Counter()
    ids = collections.Counter()
    endings = collections.Counter()
    lines_total = malformed = 0
    for folder, count in (("events", 7), ("clicks", 4), ("links", 2)):
        path = os.path.join(data, folder)
        for name in sorted(os.listdir(path)) if os.path.isdir(path) else []:
            with open(os.path.join(path, name), "rb") as f:
                text = f.read().decode("latin-1")
            endings[f"{folder}: " + ("empty" if not text else "newline at end" if text.endswith("\n\n") and False else
                                     "no newline at end" if not text.endswith("\n") else
                                     "empty last line" if text.endswith("\n\n") else "ends with newline")] += 1
            if folder != "events":
                continue
            lines = text.split("\n")
            if lines and lines[-1] == "":
                lines.pop()
            for line in lines[1:]:
                lines_total += 1
                n = len(line.encode("latin-1")) + 1
                lengths["<=63" if n <= 63 else "64" if n == 64 else "65" if n == 65 else "66-127" if n <= 127 else
                        "128" if n == 128 else "129" if n == 129 else "130-256" if n <= 256 else ">256"] += 1
                f = split_fields(line, count)
                if f is None:
                    malformed += 1
                    continue
                quoted[line.count('"') // 2] += 1
                for k, v in enumerate(f):
                    field_len[k][bucket(len(v.encode("latin-1")))] += 1
                ids[f[0]] += 1
                ts, ingest = parse_ts(f[2]), parse_ts(f[3])
                form = ("bad" if ts is None else "F3" if f[2].isdigit() else "F1" if len(f[2]) == 20 else "F2")
                if form == "F3" and len(f[2]) != 13:
                    form = "F3 padded" if len(f[2]) > 13 else "F3 short"
                forms[form] += 1
                if ts is not None and ingest is not None:
                    d = ingest - ts
                    if DAY - 1000 <= d <= DAY + 1000:
                        edge[f"ingest - ts within a second of 24 h ({form})"] += 1
                    if d > DAY and form == "F2" and f[2][:10] == f[3][:10]:
                        edge["late, F2 shows the date of ingest_ts"] += 1
                    if d <= DAY and form == "F2" and f[2][:10] != f[3][:10] and abs(int(f[2][8:10]) - int(f[3][8:10])) >= 2:
                        edge["not late, F2 date two days from ingest_ts"] += 1
    names = ["event_id", "user_id", "ts", "ingest_ts", "event_type", "product_id", "price_cents"]
    print(f"{data}: {lines_total} event lines, {malformed} malformed, {len(ids)} ids, {sum(1 for c in ids.values() if c > 1)} repeated, most copies {max(ids.values(), default=0)}")
    print("  line bytes:", dict(sorted(lengths.items())))
    print("  quoted fields per line:", dict(sorted(quoted.items())))
    print("  ts forms:", dict(forms))
    for name, counter in zip(names, field_len):
        print(f"  {name} bytes:", dict(sorted(counter.items(), key=lambda kv: (len(kv[0]), kv[0]))))
    print("  edges:", dict(edge))
    print("  files:", dict(endings))


if __name__ == "__main__":
    main()
