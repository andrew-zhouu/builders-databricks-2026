#!/usr/bin/env python3
"""
How sharp is a tssweep set (tools/tssweep.py) as a test of timestamp and price
decoding? One value of one line is changed by a small amount (a ts by 1 ms, by
1 s, a price by 1 cent), the Python reference is run again, and the answer is
compared with the one before: the share of such single errors that change the
answer is what a pipeline's wrong decode of one value would be caught with,
without any self-check.

Usage: sweepsens.py SEED [USERS] [TRIALS]
Light: a few hundred runs of the reference on a small set.
"""
import os
import random
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import mutate  # noqa: E402  (parse_ts, split_fields)


def answer(data, out):
    shutil.rmtree(out, ignore_errors=True)
    subprocess.run([sys.executable, "-I", os.path.join(HERE, "..", "reference", "reference.py"), data, out], check=True, capture_output=True)
    return tuple(open(os.path.join(out, name), "rb").read() for name in ("user_daily.csv", "campaign_daily.csv"))


def main():
    seed = int(sys.argv[1])
    users = int(sys.argv[2]) if len(sys.argv) > 2 else 400
    trials = int(sys.argv[3]) if len(sys.argv) > 3 else 60
    R = random.Random(seed)
    work = tempfile.mkdtemp(prefix="sweepsens-")
    data = os.path.join(work, "d")
    subprocess.run([sys.executable, "-I", os.path.join(HERE, "tssweep.py"), str(seed), data, str(users)], check=True)
    base = answer(data, os.path.join(work, "r"))
    files = {}
    for folder, count, at in (("events", 7, 2), ("clicks", 4, 3)):
        for name in sorted(os.listdir(os.path.join(data, folder))):
            path = os.path.join(data, folder, name)
            lines = open(path, "rb").read().decode("latin-1").split("\n")
            files[path] = (lines, count, at)
    for label, folder, column, delta in (("event ts + 1 ms", "events", 2, 1), ("event ts - 1 ms", "events", 2, -1), ("event ts + 1 s", "events", 2, 1000),
                                         ("click ts + 1 ms", "clicks", 3, 1), ("click ts - 1 ms", "clicks", 3, -1), ("price + 1 cent", "events", 6, 1)):
        changed = tried = 0
        paths = [p for p in files if os.sep + folder + os.sep in p]
        while tried < trials:
            path = R.choice(paths)
            lines, count, _ = files[path]
            number = R.randrange(1, len(lines))
            f = mutate.split_fields(lines[number], count)
            if f is None:
                continue
            if column == 6:
                if f[4] != "purchase" or not f[6].isdigit() or not 1 <= int(f[6]) < 10_000_000:
                    continue
                f[6] = str(int(f[6]) + delta)
            else:
                ms = mutate.parse_ts(f[column])
                if ms is None:
                    continue
                f[column] = str(ms + delta)
            tried += 1
            new = list(lines)
            new[number] = ",".join(f)
            with open(path, "wb") as out:
                out.write("\n".join(new).encode("latin-1"))
            changed += answer(data, os.path.join(work, "r")) != base
            with open(path, "wb") as out:
                out.write("\n".join(lines).encode("latin-1"))
        print(f"  {label}: {changed} of {tried} single errors change the answer")
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
