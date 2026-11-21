#!/usr/bin/env python3
"""
Slow, literal reference implementation of rules.md (Shopping Sessions).

Written for clarity, not speed: it is the oracle the fast pipeline is diffed
against on fixtures, the dev set and extra generated/adversarial datasets.

Usage: reference.py DATA_DIR OUT_DIR      (also honours $DATA_DIR / $OUTPUT_DIR)
Writes OUT_DIR/user_daily.csv and OUT_DIR/campaign_daily.csv in the layout of
the published expected CSVs, plus OUT_DIR/{user_daily,campaign_daily}.tsv-ish
raw-bytes files are not needed: IDs are written as raw bytes.
"""
import glob
import json
import os
import sys

DAY = 86_400_000
GAP = 1_800_000
REACH = 7 * DAY


def read_lines(path):
    """Data lines of one CSV: split on \\n only, first line (header) skipped."""
    with open(path, "rb") as f:
        data = f.read()
    if not data:
        return []
    lines = data.split(b"\n")
    if lines[-1] == b"":      # the file ended with \n: no extra empty line after it
        lines.pop()
    return lines[1:]


def fields(line, count):
    """Split by the rules' dialect; None when the line is malformed."""
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


def all_digits(b):
    return len(b) > 0 and all(0x30 <= c <= 0x39 for c in b)


def days_from_civil(y, m, d):
    y -= m <= 2
    era = y // 400
    yoe = y - era * 400
    doy = (153 * ((m + 9) % 12) + 2) // 5 + d - 1
    doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return era * 146097 + doe - 719468


def civil_from_days(z):
    z += 719468
    era = z // 146097
    doe = z - era * 146097
    yoe = (doe - doe // 1460 + doe // 36524 - doe // 146096) // 365
    y = yoe + era * 400
    doy = doe - (365 * yoe + yoe // 4 - yoe // 100)
    mp = (5 * doy + 2) // 153
    d = doy - (153 * mp + 2) // 5 + 1
    m = mp + 3 if mp < 10 else mp - 9
    return (y + (m <= 2), m, d)


def days_in_month(y, m):
    if m == 2:
        leap = y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)
        return 29 if leap else 28
    return 30 if m in (4, 6, 9, 11) else 31


def parse_ts(b):
    """Epoch milliseconds for a valid F1/F2/F3 timestamp, else None."""
    if all_digits(b):
        return int(b)
    n = len(b)
    if n not in (20, 25):
        return None
    for i in (0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18):
        if not 0x30 <= b[i] <= 0x39:
            return None
    if b[4:5] != b"-" or b[7:8] != b"-" or b[10:11] != b"T" or b[13:14] != b":" or b[16:17] != b":":
        return None
    y, mo, d = int(b[0:4]), int(b[5:7]), int(b[8:10])
    h, mi, s = int(b[11:13]), int(b[14:16]), int(b[17:19])
    if not (1 <= mo <= 12 and 1 <= d <= days_in_month(y, mo) and h <= 23 and mi <= 59 and s <= 59):
        return None
    ms = (days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s) * 1000
    if n == 20:
        return ms if b[19:20] == b"Z" else None
    sign = b[19:20]
    if sign not in (b"+", b"-") or b[22:23] != b":":
        return None
    for i in (20, 21, 23, 24):
        if not 0x30 <= b[i] <= 0x39:
            return None
    oh, om = int(b[20:22]), int(b[23:25])
    if oh > 23 or om > 59:
        return None
    off = (oh * 60 + om) * 60_000
    return ms - off if sign == b"+" else ms + off


class DSU:
    def __init__(self):
        self.parent = {}

    def find(self, x):
        p = self.parent
        if x not in p:
            return x
        root = x
        while p[root] != root:
            root = p[root]
        while p[x] != root:
            p[x], x = root, p[x]
        return root

    def union(self, a, b):
        p = self.parent
        p.setdefault(a, a)
        p.setdefault(b, b)
        ra, rb = self.find(a), self.find(b)
        if ra != rb:
            # the smaller ID is the root, so the root is the person's name
            if ra < rb:
                p[rb] = ra
            else:
                p[ra] = rb


def run(data_dir, out_dir):
    with open(os.path.join(data_dir, "manifest.json"), "rb") as f:
        manifest = json.load(f)
    ws = parse_ts(manifest["window_start"].encode())
    we = parse_ts(manifest["window_end"].encode())
    assert ws is not None and we is not None

    category = {}
    for line in read_lines(os.path.join(data_dir, "products.csv")):
        f = fields(line, 2)
        if f is not None:
            category[f[0]] = f[1]
    channel = {}
    for line in read_lines(os.path.join(data_dir, "campaigns.csv")):
        f = fields(line, 2)
        if f is not None:
            channel[f[0]] = f[1]

    # ---- linked IDs
    dsu = DSU()
    for path in sorted(glob.glob(os.path.join(glob.escape(data_dir), "links", "*.csv"))):
        for line in read_lines(path):
            f = fields(line, 2)
            if f is None or not f[0] or not f[1]:
                continue
            dsu.union(f[0], f[1])

    # ---- events: parse, dedup (smallest ingest_ts), validate, late
    kept = {}
    for path in sorted(glob.glob(os.path.join(glob.escape(data_dir), "events", "*.csv"))):
        for line in read_lines(path):
            f = fields(line, 7)
            if f is None or not f[0]:
                continue
            ingest = parse_ts(f[3])
            assert ingest is not None and len(f[3]) == 20, f"ingest_ts not valid F1: {line!r}"
            prev = kept.get(f[0])
            if prev is None or ingest < prev[0]:
                kept[f[0]] = (ingest, f)
            elif ingest == prev[0]:
                assert prev[1] == f, f"copies with equal ingest_ts differ: {prev[1]!r} vs {f!r}"

    per_person = {}
    for ingest, f in kept.values():
        _, user, ts_raw, _, etype, product, price_raw = f
        if not user:
            continue
        ts = parse_ts(ts_raw)
        if ts is None or not (ws <= ts < we):
            continue
        if etype not in (b"view", b"cart", b"purchase"):
            continue
        if not product:
            continue
        price = 0
        if etype == b"purchase":
            if not all_digits(price_raw):
                continue
            price = int(price_raw)
            if not (1 <= price <= 10_000_000):
                continue
        if ingest - ts > DAY:
            continue
        per_person.setdefault(dsu.find(user), []).append(
            (ts, etype == b"purchase", price, category.get(product, b"unknown")))
    del kept

    # ---- clicks: parse, dedup, validate
    clicks = {}
    for path in sorted(glob.glob(os.path.join(glob.escape(data_dir), "clicks", "*.csv"))):
        for line in read_lines(path):
            f = fields(line, 4)
            if f is None or not f[0]:
                continue
            prev = clicks.get(f[0])
            if prev is None:
                clicks[f[0]] = f
            else:
                assert prev == f, f"copies of a click differ: {prev!r} vs {f!r}"
    person_clicks = {}
    for click_id, user, campaign, ts_raw in clicks.values():
        if not user:
            continue
        ts = parse_ts(ts_raw)
        if ts is None or not (ws - REACH <= ts < we):
            continue
        if campaign not in channel:
            continue
        person_clicks.setdefault(dsu.find(user), []).append((ts, click_id, campaign))
    del clicks

    # ---- sessions, attribution, aggregation
    user_daily = {}       # (person, day) -> [sessions, events, revenue, {category: cents}]
    campaign_daily = {}   # (campaign, day) -> [sessions, purchasing, revenue, assisted]
    for person, events in per_person.items():
        events.sort(key=lambda e: e[0])
        sessions = []
        for event in events:
            if not sessions or event[0] - sessions[-1][-1][0] > GAP:
                sessions.append([event])
            else:
                sessions[-1].append(event)
        pclicks = person_clicks.get(person, ())
        for session in sessions:
            start = session[0][0]
            day = start // DAY
            revenue = sum(e[2] for e in session if e[1])
            purchasing = any(e[1] for e in session)
            row = user_daily.setdefault((person, day), [0, 0, 0, {}])
            row[0] += 1
            row[1] += len(session)
            row[2] += revenue
            for e in session:
                if e[1]:
                    row[3][e[3]] = row[3].get(e[3], 0) + e[2]
            window = [c for c in pclicks if c[0] <= start and start - c[0] <= REACH]
            if window:
                best = min(window, key=lambda c: (-c[0], c[1]))
                credited = best[2]
            else:
                credited = b"organic"
            crow = campaign_daily.setdefault((credited, day), [0, 0, 0, 0])
            crow[0] += 1
            crow[1] += purchasing
            crow[2] += revenue
            for campaign in {c[2] for c in window}:
                campaign_daily.setdefault((campaign, day), [0, 0, 0, 0])[3] += 1

    def date(day):
        y, m, d = civil_from_days(day)
        return b"%04d-%02d-%02d" % (y, m, d)

    os.makedirs(out_dir, exist_ok=True)
    with open(os.path.join(out_dir, "user_daily.csv"), "wb") as f:
        f.write(b"user_id,day,sessions,events,revenue_cents,top_category\n")
        for (person, day), (sessions, events, revenue, cats) in sorted(user_daily.items()):
            top = b""
            if cats:
                top = min(cats.items(), key=lambda kv: (-kv[1], kv[0]))[0]
            f.write(b"%s,%s,%d,%d,%d,%s\n" % (person, date(day), sessions, events, revenue, top))
    with open(os.path.join(out_dir, "campaign_daily.csv"), "wb") as f:
        f.write(b"campaign_id,channel,day,sessions,purchasing_sessions,revenue_cents,assisted_sessions\n")
        for (campaign, day), (sessions, purchasing, revenue, assisted) in sorted(campaign_daily.items()):
            chan = b"organic" if campaign == b"organic" else channel[campaign]
            f.write(b"%s,%s,%s,%d,%d,%d,%d\n" % (campaign, chan, date(day), sessions, purchasing, revenue, assisted))


if __name__ == "__main__":
    data = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("DATA_DIR", "/data")
    out = sys.argv[2] if len(sys.argv) > 2 else os.environ.get("OUTPUT_DIR", "/output")
    run(data, out)
