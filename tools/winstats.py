#!/usr/bin/env python3
"""
Where the fields of a data set's lines end, as a splitter with 64-byte windows
sees them: for event lines the byte of the first comma, the bytes from there
to the third comma, and from there to the newline; for click lines the byte of
the first comma, the bytes from there to the second, and from there to the
newline. For each of these distances: how many lines have it at 58 to 68 (one
count per value), how many beyond, and the largest. Lines with another number
of commas are counted apart, and so are lines that follow a line of 150 bytes
or more and are shorter than 12.

Usage: winstats.py DATA_DIR
"""
import os
import sys


def show(name, values):
    near = {v: 0 for v in range(58, 69)}
    beyond = 0
    for v in values:
        if v in near:
            near[v] += 1
        elif v > 68:
            beyond += 1
    print(f"  {name}: " + " ".join(f"{v}:{n}" for v, n in near.items()) + f"; beyond 68: {beyond}; largest {max(values, default=0)}")


def main():
    data = sys.argv[1]
    for folder, commas in (("events", 6), ("clicks", 3)):
        path = os.path.join(data, folder)
        first, middle, rest, lengths = [], [], [], []
        other = after_long = lines = 0
        for name in sorted(os.listdir(path)) if os.path.isdir(path) else []:
            if not name.endswith(".csv"):
                continue
            with open(os.path.join(path, name), "rb") as f:
                rows = f.read().split(b"\n")[1:]
            if rows and rows[-1] == b"":
                rows.pop()
            before = 0
            for row in rows:
                lines += 1
                lengths.append(len(row))
                if before >= 150 and len(row) < 12:
                    after_long += 1
                before = len(row)
                at = [i for i, c in enumerate(row) if c == 0x2C]
                if len(at) != commas:
                    other += 1
                    continue
                cut = at[2] if folder == "events" else at[1]
                first.append(at[0])
                middle.append(cut - at[0])
                rest.append(len(row) - cut)
        print(f"{folder}: {lines} lines, {other} with another number of commas, {after_long} short lines right after a line of 150 bytes or more, "
              f"{sum(1 for n in lengths if 180 <= n <= 260)} lines of 180 to 260 bytes")
        show("first comma at byte", first)
        show("from the first comma to the " + ("third" if folder == "events" else "second"), middle)
        show("from there to the newline", rest)


if __name__ == "__main__":
    main()
