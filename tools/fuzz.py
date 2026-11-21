#!/usr/bin/env python3
"""
Adversarial dataset generator for differential testing.

Writes a small dataset in the /data layout whose rows lean on every corner of
rules.md: all timestamp forms and near-miss bad forms, window and 7-day edges,
30-minute gaps to the millisecond, duplicate events whose copies disagree,
irregular IDs (case, spaces, long, non-ASCII), odd prices, quoting, malformed
lines, \\r, empty lines, long lines, links in chains and stars.

It keeps the guarantees the rules give about the data: ingest_ts is always a
valid F1 timestamp, copies of an event with equal ingest_ts are identical, and
copies of a click are identical.

Usage: fuzz.py SEED OUT_DIR [ROWS]
"""
import datetime
import json
import os
import random
import sys

DAY = 86_400_000
EPOCH = datetime.datetime(1970, 1, 1)


def f1(ms):
    return (EPOCH + datetime.timedelta(milliseconds=ms)).strftime("%Y-%m-%dT%H:%M:%SZ")


def f2(ms, off_min):
    local = EPOCH + datetime.timedelta(milliseconds=ms + off_min * 60_000)
    sign = "-" if off_min < 0 else "+"
    a = abs(off_min)
    return local.strftime("%Y-%m-%dT%H:%M:%S") + f"{sign}{a // 60:02d}:{a % 60:02d}"


def main():
    seed = int(sys.argv[1])
    out = sys.argv[2]
    rows = int(sys.argv[3]) if len(sys.argv) > 3 else 4000
    R = random.Random(seed)

    # ---- window: a random day 2025-2028 (sometimes not at midnight, sometimes
    # around New Year or the leap day), 14 days long, sometimes shorter
    pick = R.random()
    if pick < 0.2:
        start_day = (datetime.date(R.choice([2025, 2026, 2027]), 12, R.randint(20, 31)) - datetime.date(1970, 1, 1)).days
    elif pick < 0.4:
        start_day = (datetime.date(2028, 2, R.randint(16, 29)) - datetime.date(1970, 1, 1)).days
    else:
        start_day = (datetime.date(2025, 1, 1) - datetime.date(1970, 1, 1)).days + R.randrange(1400)
    ws = start_day * DAY
    if R.random() < 0.2:
        ws += R.randrange(24 * 3600) * 1000
    wlen = 14 * DAY if R.random() < 0.8 else R.randint(1, 16) * DAY
    we = ws + wlen

    # ---- lookups
    cats = ["shoes", "books", "toys", "unknown" if R.random() < 0.2 else "garden", "Books", "a", "zz"]
    products = {f"p{i}": R.choice(cats) for i in range(1, 40)}
    for odd in ["p007", "P5", "p 1", "p4194305", "p12345678", "x", "q1", "p0"]:
        if R.random() < 0.7:
            products[odd] = R.choice(cats)
    campaigns = {f"c_{i}": R.choice(["search", "social", "email"]) for i in range(1, 25)}
    for odd in ["c_007", "C_30", "c_4194305", "c_12345678", "camp", "c_0", "c_", "c_3x"]:
        if R.random() < 0.7:
            campaigns[odd] = R.choice(["search", "social", "email", "odd"])

    # ---- IDs
    def reg(prefix, n):
        return prefix + "".join(R.choice("0123456789abcdefghijklmnopqrstuvwxyz") for _ in range(n))

    users = [reg("u", R.choice([1, 2, 3, 7, 8, 8, 8])) for _ in range(max(8, rows // 25))]
    users += ["U1", "u1", " u1", "u1 ", "u_1", "uzzzzzzzzz", "u123456789a", "ué1", "0", "z", "9zzzzzzzz", "A\tB",
              "user_id", "u1\r", "éé"]
    R.shuffle(users)

    def user():
        r = R.random()
        if r < 0.02:
            return ""
        return R.choice(users) if r < 0.9 else R.choice(users[:6])

    # ---- timestamps
    offsets = [-600, -570, -300, -1, 0, 1, 60, 330, 345, 720, 1439, -1439]

    def good_ts(ms):
        r = R.random()
        if ms % 1000 or r < 0.3:
            z = R.choice([0, 0, 0, 1, 3, 7, 12]) if R.random() < 0.15 else 0
            return "0" * z + str(ms)
        if r < 0.45:
            return f2(ms, R.choice(offsets))
        return f1(ms)

    def bad_ts(ms):
        s = f1(ms - ms % 1000)
        forms = [
            "", s[:19], s[:19] + "z", s.replace("T", "t"), s.replace("T", " "), s[:19] + ".123Z", f"{ms}.0",
            "1.79e12", "-5", "+5", s[:5] + "13" + s[7:], s[:5] + "02-30" + s[10:], s[:5] + "02-29" + s[10:],
            s[:5] + "00" + s[7:], s[:8] + "00" + s[10:], s[:8] + "32" + s[10:], s[:11] + "24" + s[13:],
            s[:14] + "60" + s[16:], s[:17] + "60Z", s[:19] + "+24:00", s[:19] + "+05:60", s[:19] + "+0200",
            s[:19] + "-0:300", s[:19] + "+02:0", s[:19] + "+02:000", s[:19] + " 02:00", s[:19] + "+2:00",
            "٢٠٢٦" + s[4:], s[:3] + "２" + s[4:], str(ms) + " ", " " + str(ms), str(ms)[:-1] + "x",
            "9" * 19, "9" * 20, "9" * 25, "9" * 40, "1" + "0" * 16, "0", "86400000", s[:19] + "Y", s + "Z",
            "x" + s[1:], s[:4] + "/" + s[5:], s[:19] + "+", s[:10] + "T" + s[11:13] + "-" + s[14:],
            f1(ws - DAY * 400), f1(ws + DAY * 800), f2(ws + DAY * 800, 60), str(ws - 1), str(we), f1(we),
            f1(ws - 1000), "0" * 20, "0" * 25, "1" * 20, s[:19] + "-00:00x", s.lower(),
        ]
        return R.choice(forms)

    def ts_field(ms):
        r = R.random()
        if r < 0.07:
            return bad_ts(ms)
        if r < 0.12:  # at or just outside the window
            return good_ts(R.choice([ws - 1, ws, we - 1, we, ws - 1000, we + 1000, ws + 1]))
        if r < 0.14:  # digits-only strings of awkward lengths
            return str(ms).zfill(R.choice([16, 17, 20, 21, 25, 26, 30]))
        return good_ts(ms)

    # ---- events: people with sessions, gaps on the 30-minute edge
    events = []          # field lists
    eid = 0

    def new_eid():
        nonlocal eid
        eid += 1
        r = R.random()
        if r < 0.93:
            return "e" + reg("", R.choice([1, 2, 5, 7, 8, 8, 8]))
        return R.choice([f"E{eid}", f"e{eid} ", f" e{eid}", f"e{eid}\r", f"x{eid}", f"e{eid:010d}", "e", f"e_{eid}", f"é{eid}"])

    def price_field(purchase):
        r = R.random()
        if purchase and r < 0.75:
            return str(R.choice([1, 100, 250, 999, 10_000_000, R.randint(1, 50000)]))
        return R.choice(["", "", "250", "0250", "0000000000000000000250", "0", "00", "10000000", "10000001",
                         "010000000", "19.99", "-5", "+500", "1e3", "250000000", "２５０", "٢٥٠", " 5", "5 ",
                         "99999999999999999999", "1" * 17, "12a", "1000\r"])

    def ingest_for(ts_ms):
        base = ts_ms - ts_ms % 1000
        r = R.random()
        if r < 0.05:
            return base + DAY if ts_ms % 1000 == 0 else base + DAY + 1000
        if r < 0.10:
            return base + DAY + 1000
        if r < 0.13:
            return base - R.randint(1, 5) * 1000
        if r < 0.16:
            return base + R.randint(DAY // 1000 + 1, 5 * DAY // 1000) * 1000
        if r < 0.17:
            return base + DAY + (1000 if ts_ms % 1000 else 0)
        return base + 1000 + R.randint(0, 600) * 1000

    t = ws
    cur_user = user()
    n_events = rows
    while len(events) < n_events:
        r = R.random()
        if r < 0.25:
            cur_user = user()
            t = ws + R.randrange(max(1, wlen // 1000)) * 1000 + (R.randrange(1000) if R.random() < 0.1 else 0)
        elif r < 0.35:
            # the session gap to the millisecond, and the sessionizer's internal
            # 2^20 ms slot width (pass3.rs) to the millisecond
            t += R.choice([1_800_000, 1_800_001, 1_799_999, 0, 0, 1, 999, 1000,
                           1_048_575, 1_048_576, 1_048_577, 751_424, 751_425, 2_097_152])
        elif r < 0.40:
            t = (t // DAY + 1) * DAY - R.choice([1, 1000, 60_000, 0])   # around midnight
        else:
            t += R.choice([0, 1000, 5000, 60_000, 600_000, 1_200_000])
        ts_ms = min(t, we + 5000)
        kind = R.random()
        etype = "purchase" if kind < 0.25 else ("cart" if kind < 0.4 else "view")
        if R.random() < 0.03:
            etype = R.choice(["Purchase", "PURCHASE", "View", "cart ", " view", "click", "", "views", "purchas", "viewx", "purchase\r"])
        product = R.choice(list(products) + ["p999", "", "P1", "p1 "]) if R.random() < 0.9 else R.choice(["p1", "p2"])
        row = [new_eid(), cur_user, ts_field(ts_ms), f1(ingest_for(ts_ms)), etype, product, price_field(etype == "purchase")]
        events.append(row)
        # duplicates: later copies, some with other users / fixed or broken ts
        if R.random() < 0.12:
            base_ing = ingest_for(ts_ms)
            row[3] = f1(base_ing)
            used = {base_ing}
            for _ in range(R.randint(1, 3)):
                copy = list(row)
                ing = base_ing + R.choice([-600, -1, 1, 1, 60, 600, 86400]) * 1000
                if ing in used:
                    continue
                used.add(ing)
                copy[3] = f1(ing)
                m = R.random()
                if m < 0.25:
                    copy[1] = user()
                elif m < 0.45:
                    copy[2] = ts_field(ts_ms + R.choice([0, 1000, -1000, 3_600_000]))
                elif m < 0.55:
                    copy[4] = R.choice(["view", "purchase", "bad"])
                elif m < 0.65:
                    copy[6] = price_field(True)
                elif m < 0.70:
                    copy[5] = R.choice(["", "p1", "p999"])
                events.append(copy)
            if R.random() < 0.3:
                events.append(list(R.choice(events[-3:])))   # an identical copy (same ingest)
        # an event whose earliest copy is invalid: every later, valid copy must go with
        # it, which can leave a person with purchases but no events at all
        if R.random() < 0.04:
            ing = ingest_for(ts_ms)
            lone = "u" + reg("", 8)
            bad = [new_eid(), lone, bad_ts(ts_ms), f1(ing), "purchase", "p1", "500"]
            events.append(bad)
            for k in range(R.randint(1, 3)):
                events.append([bad[0], lone if R.random() < 0.7 else user(), good_ts(ts_ms), f1(ing + (k + 1) * 60000),
                               R.choice(["purchase", "purchase", "view"]), "p1", "500"])

    # identical copies must stay identical; copies with equal ingest and different
    # content are not allowed by the rules: keep the first row per (id, ingest)
    seen = {}
    cleaned = []
    for row in events:
        key = (row[0], row[3])
        if key in seen and seen[key] != row:
            continue
        seen[key] = row
        cleaned.append(row)
    events = cleaned

    # ---- clicks: at the edges of sessions' 7-day reach, ties in the same ms
    clicks = {}
    kid = 0
    starts = [(row[1], i) for i, row in enumerate(events)]
    for _ in range(max(4, rows // 3)):
        kid += 1
        r = R.random()
        cid = "k" + reg("", R.choice([1, 2, 7, 8, 8])) if r < 0.9 else R.choice(
            [f"K{kid}", f"k{kid} ", f"k_{kid}", f"k{kid:012d}", "k", f"é{kid}", f" k{kid}"])
        u = R.choice(users) if R.random() < 0.85 else user()
        base = ws + R.randrange(max(1, wlen // 1000)) * 1000
        delta = R.choice([0, 1, -1, 7 * DAY, 7 * DAY + 1, 7 * DAY - 1, 1000, 3600_000, 2 * DAY, 6 * DAY, 8 * DAY])
        ms = base - delta
        r = R.random()
        if r < 0.05:
            tsf = bad_ts(ms)
        elif r < 0.10:
            tsf = good_ts(R.choice([ws - 7 * DAY, ws - 7 * DAY - 1, ws - 7 * DAY + 1, we, we - 1]))
        else:
            tsf = good_ts(ms)
        camp = R.choice(list(campaigns)) if R.random() < 0.92 else R.choice(["c_999", "C_1", "c_01", "", "organic", "c_1 "])
        clicks[cid] = [cid, u, camp, tsf]
        if R.random() < 0.15:   # same person, same millisecond, another campaign
            kid += 1
            cid2 = "k" + reg("", R.choice([1, 2, 8])) if R.random() < 0.8 else f"K{kid}"
            clicks.setdefault(cid2, [cid2, u, R.choice(list(campaigns)), tsf if R.random() < 0.8 else good_ts(ms)])

    # ---- links: trees, chains, stars, repeats, self-links, dirt
    links = []
    pool = users + [reg("u", 8) for _ in range(len(users) // 3)]
    for _ in range(len(users) // 2):
        a, b = R.choice(pool), R.choice(pool)
        links.append([a, b])
        if R.random() < 0.1:
            links.append([b, a])
        if R.random() < 0.05:
            links.append([a, a])
    chain = R.sample(pool, min(len(pool), R.randint(2, 30)))
    links += [[x, y] for x, y in zip(chain, chain[1:])]
    hub = R.choice(pool)
    links += [[hub, R.choice(pool)] for _ in range(R.randint(0, 20))]
    for bad in [["u1", ""], ["", "u3"], ["", ""]]:
        if R.random() < 0.5:
            links.append(bad)

    # ---- CSV writing with quoting and malformed lines
    def encode(fields, nfields):
        parts = []
        for f in fields:
            parts.append('"' + f + '"' if R.random() < 0.2 else f)
        line = ",".join(parts)
        m = R.random()
        if m < 0.04:
            fault = R.randrange(12)
            if fault == 0:
                line = line[:line.rfind(",")] if "," in line else ""
            elif fault == 1:
                line += ",x"
            elif fault == 2:
                line = '"' + line
            elif fault == 3:
                line += '"'
            elif fault == 4:
                line = line[line.find(","):] if "," in line else line
            elif fault == 5:
                return [line, ""]                      # an extra empty line
            elif fault == 6:
                at = R.randrange(len(line) + 1)
                line = line[:at] + '"' + line[at:]    # a stray quote anywhere
            elif fault == 7:
                at = R.randrange(len(line) + 1)
                line = line[:at] + "," + line[at:]    # an extra comma anywhere
            elif fault == 8:
                line = line + "\r"
            elif fault == 9:
                line = line.replace(",", ",,", 1)
            elif fault == 10:
                parts[R.randrange(len(parts))] = '""'
                line = ",".join(parts)
            else:
                at = R.randrange(len(parts))
                parts[at] = '"' + parts[at].replace('"', "") + '""'
                line = ",".join(parts)
        return [line]

    def split_fields(line, count):
        parts = line.split(",")
        if len(parts) != count:
            return None
        res = []
        for part in parts:
            if '"' in part:
                if len(part) >= 2 and part[0] == '"' and part[-1] == '"' and '"' not in part[1:-1]:
                    part = part[1:-1]
                else:
                    return None
            res.append(part)
        return res

    def valid_f1(s):
        if len(s) != 20 or s[19] != "Z" or not all(c in "0123456789" for c in s[0:4] + s[5:7] + s[8:10] + s[11:13] + s[14:16] + s[17:19]):
            return False
        try:
            datetime.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ")
            return True
        except ValueError:
            return False

    def keep_contract(folder, lines):
        """Drop lines that break what the rules promise about the data: an
        ingest_ts that is not valid F1, copies of an event with equal ingest_ts
        but different values, copies of a click with different values."""
        if folder == "links":
            return lines
        kept, seen = [], {}
        for line in lines:
            f = split_fields(line, 7 if folder == "events" else 4)
            if f is not None and f[0] != "":
                if folder == "events":
                    if not valid_f1(f[3]):
                        continue
                    key = (f[0], f[3])
                else:
                    key = f[0]
                if seen.setdefault(key, f) != f:
                    continue
            kept.append(line)
        return kept

    def write(folder, header, rows_, nfields, nfiles):
        os.makedirs(os.path.join(out, folder), exist_ok=True)
        lines = []
        for row in rows_:
            lines += encode(row, nfields)
        if R.random() < 0.3:
            lines.append(header)                  # a data row equal to the header
        R.shuffle(lines)
        lines = keep_contract(folder, lines)
        cuts = sorted(R.randrange(len(lines) + 1) for _ in range(nfiles - 1))
        cuts = [0] + cuts + [len(lines)]
        for i in range(nfiles):
            chunk = lines[cuts[i]:cuts[i + 1]]
            if False:
                chunk = [header] + chunk
            body = header + "\n" + "".join(l + "\n" for l in chunk)
            r = R.random()
            if r < 0.15 and chunk:
                body = body[:-1]                  # no trailing newline
            elif r < 0.2:
                body += "\n"                      # a trailing empty line
            with open(os.path.join(out, folder, f"part-{i:05d}.csv"), "wb") as f:
                f.write(body.encode("utf-8"))

    os.makedirs(out, exist_ok=True)
    write("events", "event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents", events, 7, R.randint(1, 5))
    write("clicks", "click_id,user_id,campaign_id,ts", list(clicks.values()), 4, R.randint(1, 4))
    write("links", "user_id_a,user_id_b", links, 2, R.randint(1, 3))
    with open(os.path.join(out, "products.csv"), "w", encoding="utf-8", newline="") as f:
        f.write("product_id,category\n" + "".join(f"{k},{v}\n" for k, v in products.items()))
    with open(os.path.join(out, "campaigns.csv"), "w", encoding="utf-8", newline="") as f:
        f.write("campaign_id,channel\n" + "".join(f"{k},{v}\n" for k, v in campaigns.items()))
    with open(os.path.join(out, "manifest.json"), "w") as f:
        json.dump({"window_start": f1(ws), "window_end": f1(we)}, f)


if __name__ == "__main__":
    main()
