#!/usr/bin/env python3
"""
What IDs a data set holds, for checking that a generated or mutated set really
has the shapes a test is meant to have: for event IDs, the users of events,
click IDs, the users of clicks and both sides of links, how many distinct IDs
are of the long form (a letter and 32 lowercase hex digits), how many are near
it (32 to 34 bytes, or 33 bytes that are not the form), the longest, and how
many share their first 17 bytes with another; and how many lines of each folder
are longer than 64, 128 and 255 bytes.

Usage: idstats.py DATA_DIR
"""
import os
import sys

HEX = set(b"0123456789abcdef")


def fields(line, count):
    parts = line.split(b",")
    if len(parts) != count:
        return None
    out = []
    for part in parts:
        if b'"' in part:
            if len(part) >= 2 and part[:1] == b'"' and part[-1:] == b'"' and b'"' not in part[1:-1]:
                part = part[1:-1]
            else:
                return None
        out.append(part)
    return out


def describe(name, ids):
    ids = {i for i in ids if i}
    long_form = [i for i in ids if len(i) == 33 and all(c in HEX for c in i[1:])]
    near = [i for i in ids if 32 <= len(i) <= 34 and i not in set(long_form)]
    heads = {}
    for i in ids:
        if len(i) >= 17:
            heads[i[:17]] = heads.get(i[:17], 0) + 1
    shared = sum(n for n in heads.values() if n > 1)
    odd = sum(1 for i in ids if any(c < 0x20 or c >= 0x7f for c in i))
    print(f"  {name}: {len(ids)} distinct, {len(long_form)} long form, {len(near)} near it (32..34 bytes), "
          f"{shared} sharing 17 first bytes, {odd} with a control or non-ASCII byte, longest {max(map(len, ids), default=0)}")


def main():
    data = sys.argv[1]
    for folder, count, columns in (("events", 7, {"event_id": 0, "user_id": 1}), ("clicks", 4, {"click_id": 0, "user_id": 1}),
                                   ("links", 2, {"user_id_a": 0, "user_id_b": 1})):
        path = os.path.join(data, folder)
        ids = {name: set() for name in columns}
        lines = bad = 0
        over = [0, 0, 0]
        # The longest stretch of consecutive lines whose column is 33 bytes
        # long, and how many stretches reach eight lines.
        longest = {name: 0 for name in columns}
        eights = {name: 0 for name in columns}
        for name in sorted(os.listdir(path)) if os.path.isdir(path) else []:
            if not name.endswith(".csv"):
                continue
            with open(os.path.join(path, name), "rb") as f:
                rows = f.read().split(b"\n")[1:]
            if rows and rows[-1] == b"":
                rows.pop()
            run = {name: 0 for name in columns}
            for row in rows:
                lines += 1
                for k, limit in enumerate((64, 128, 255)):
                    over[k] += len(row) > limit
                f = fields(row, count)
                if f is None:
                    bad += 1
                for column, at in columns.items():
                    if f is not None:
                        ids[column].add(f[at])
                    if f is not None and len(f[at]) == 33:
                        run[column] += 1
                        eights[column] += run[column] == 8
                        longest[column] = max(longest[column], run[column])
                    else:
                        run[column] = 0
        print(f"{folder}: {lines} lines, {bad} malformed, longer than 64/128/255 bytes: {over[0]}/{over[1]}/{over[2]}")
        for column in columns:
            describe(column, ids[column])
            if folder != "links":
                print(f"    consecutive lines with a 33-byte {column}: longest stretch {longest[column]}, stretches of 8 or more: {eights[column]}")


if __name__ == "__main__":
    main()
