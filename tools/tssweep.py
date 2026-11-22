#!/usr/bin/env python3
"""
A data set that turns a wrong millisecond in a decoded timestamp, or a wrong
cent in a decoded price, into a wrong answer. For a pipeline that decodes
digits several lines at a time, where a lost carry looks like nothing.

Every user has two events exactly 30 minutes apart, or 30 minutes and one
millisecond, or a second more or less: one session or two. The two are written
in different forms (F1; F2 at any offset from -23:59 to +23:59; F3 plain and
with leading zeros up to 14..20 digits), so an error in one form does not
cancel. A third event, at the first one's time, is a purchase whose price runs
through every length from 1 to 8 digits, with and without leading zeros, and
through the values that are too large (some of them small again once their top
bits are gone). A click of the user sits exactly at the session's start, a
millisecond after it, exactly 7 days before it, or a millisecond more than
that: credited or not. The times run through every second of the clock and
sit on the midnights of the window, and the window is put (by the seed) around
a leap day, a year's end, the end of February in an ordinary year, or the end
of a month of 30 or 31 days, so the dates on either side of those are in it.

Usage: tssweep.py SEED OUT_DIR [USERS]      (the arguments of tools/fuzz.py)
The Python reference says what the answer is.
"""
import datetime
import json
import os
import random
import sys

DAY = 86_400_000
GAP = 1_800_000
WEEK = 7 * DAY
EPOCH = datetime.datetime(1970, 1, 1)
WINDOWS = ["2028-02-22", "2027-12-25", "2026-02-20", "2025-10-25", "2026-04-24", "2025-12-26", "2028-02-16", "2027-02-21",
           "2025-01-01", "2028-12-18", "2026-06-23", "2027-08-25"]


def f1(ms):
    return (EPOCH + datetime.timedelta(milliseconds=ms)).strftime("%Y-%m-%dT%H:%M:%SZ")


def f2(ms, minutes):
    local = EPOCH + datetime.timedelta(milliseconds=ms + minutes * 60_000)
    a = abs(minutes)
    return local.strftime("%Y-%m-%dT%H:%M:%S") + f"{'-' if minutes < 0 else '+'}{a // 60:02d}:{a % 60:02d}"


def main():
    seed, out = int(sys.argv[1]), sys.argv[2]
    users = int(sys.argv[3]) if len(sys.argv) > 3 else 3000
    R = random.Random(seed)
    start = datetime.datetime.strptime(WINDOWS[seed % len(WINDOWS)], "%Y-%m-%d")
    ws = int((start - EPOCH).total_seconds()) * 1000
    we = ws + 14 * DAY
    products = {f"p{i}": R.choice(["shoes", "books", "toys", "garden"]) for i in range(1, 30)}
    campaigns = {f"c_{i}": R.choice(["search", "social", "email"]) for i in range(1, 20)}
    special = [840, -720, 345, 330, 525, -570, 1, -1, 59, -59, 1439, -1439, 0, 765, 825, -690, 600, -600]

    def spell(ms, form=None):
        """A valid spelling of the instant; whole seconds can take any form."""
        forms = ["f3", "f3pad"] if ms % 1000 else ["f1", "f2", "f2", "f3", "f3pad"]
        form = form if form in forms else R.choice(forms)
        if form == "f1":
            return f1(ms), form
        if form == "f2":
            minutes = R.choice(special) if R.random() < 0.3 else R.choice([-1, 1]) * (R.randrange(24) * 60 + R.randrange(60))
            text = f2(ms, minutes)
            return (text[:19] + "-00:00" if minutes == 0 and R.random() < 0.5 else text), form
        if form == "f3":
            return str(ms), form
        return str(ms).zfill(R.choice([14, 15, 16, 16, 17, 18, 19, 20])), form

    def price():
        r = R.random()
        if r < 0.25:
            value = R.choice([1, 9, 10, 99, 100, 999, 1000, 9999, 10000, 99999, 100000, 999999, 1000000, 9999999, 10000000])
        elif r < 0.85:
            k = R.randint(1, 8)
            value = R.randrange(10 ** (k - 1), 10 ** k) if k < 8 else R.choice([10000000, 10000000, R.randrange(10 ** 7, 10 ** 8)])
        else:
            value = R.choice([0, 10000001, 99999999, 100000000, (1 << 32) + 250, (1 << 64) + 250, 1 << 32, 4294967295,
                              16777216 + 300, (1 << 31) + 5, 10 ** 16])
        text = str(value)
        return "0" * R.choice([0, 0, 0, 1, 2, max(0, 8 - len(text)), max(0, 16 - len(text)), R.randint(0, 12)]) + text

    days = list(range(ws, we, DAY))
    events, clicks = [], []
    for i in range(users):
        user = f"u{i:x}" if i % 8 else "u" + "".join(R.choice("0123456789abcdef") for _ in range(32))
        r = R.random()
        if r < 0.30:
            t = R.choice(days) + R.choice([0, 1000, -1000, -GAP, -GAP - 1000, -GAP + 1000, 3_600_000, -3_600_000, 1, -1])
        elif r < 0.85:
            t = R.choice(days) + R.randrange(86400) * 1000
        else:
            t = R.choice(days) + R.randrange(DAY)
        d = R.choice([0, 0, 1, 1, 1000, -1000, -1, 2, 999])
        t = min(max(t, ws), we - GAP - 2000)
        t2 = t + GAP + d
        a, form = spell(t)
        b, _ = spell(t2, R.choice([f for f in ("f1", "f2", "f3", "f3pad") if f != form]))
        late = R.choice([5, 60, 86400, 86400, 86401, 3600])
        events.append([f"e{i:x}a", user, a, f1(t - t % 1000 + (1000 if t % 1000 else 0) + late * 1000), R.choice(["view", "cart"]), "p1", ""])
        events.append([f"e{i:x}b", user, b, f1(t2 - t2 % 1000 + 1000 + R.choice([5, 60, 3600]) * 1000), "view", "p2", R.choice(["", "250"])])
        if R.random() < 0.7:
            c, _ = spell(t)
            events.append([f"e{i:x}c", user, c, f1(t - t % 1000 + 1000 + 30_000), "purchase", f"p{R.randint(1, 29)}", price()])
        if R.random() < 0.8:
            at = t + R.choice([0, 0, 1, 1, -1, -WEEK, -WEEK, -WEEK - 1, -WEEK - 1, -WEEK + 1, -1000, 1000, -3_600_000])
            text, _ = spell(at)
            clicks.append([f"k{i:x}", user, f"c_{R.randint(1, 19)}", text])
            if R.random() < 0.3:
                # A second click a millisecond or a second earlier: the later one is credited.
                text, _ = spell(at - R.choice([1, 1000, 1, 60_000]))
                clicks.append([f"k{i:x}x", user, f"c_{R.randint(1, 19)}", text])

    def write(folder, header, rows, files):
        os.makedirs(os.path.join(out, folder), exist_ok=True)
        lines = [",".join('"' + v + '"' if R.random() < 0.2 else v for v in row) for row in rows]
        R.shuffle(lines)
        for k in range(files):
            part = lines[k::files]
            with open(os.path.join(out, folder, f"part-{k:05d}.csv"), "wb") as f:
                f.write((header + "\n" + "".join(l + "\n" for l in part)).encode())

    os.makedirs(out, exist_ok=True)
    write("events", "event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents", events, R.randint(1, 4))
    write("clicks", "click_id,user_id,campaign_id,ts", clicks, R.randint(1, 3))
    write("links", "user_id_a,user_id_b", [], 1)
    with open(os.path.join(out, "products.csv"), "w") as f:
        f.write("product_id,category\n" + "".join(f"{k},{v}\n" for k, v in products.items()))
    with open(os.path.join(out, "campaigns.csv"), "w") as f:
        f.write("campaign_id,channel\n" + "".join(f"{k},{v}\n" for k, v in campaigns.items()))
    with open(os.path.join(out, "manifest.json"), "w") as f:
        json.dump({"window_start": f1(ws), "window_end": f1(we)}, f)


if __name__ == "__main__":
    main()
