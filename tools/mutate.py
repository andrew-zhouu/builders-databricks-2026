#!/usr/bin/env python3
"""
Mutations of a generated data set, aimed at a pipeline that decodes event IDs
to the generator's dense index (IDs are `e` + base 36 of a 40-bit bijection of
the index), de-duplicates by index and builds sessions from events in index
order. Every mutation makes input on which a shortcut of that kind could go
wrong while the rules (rules.md) stay perfectly well defined.

The output keeps what the rules promise about the data: ingest_ts is a valid
F1 timestamp, copies of an event with equal ingest_ts are identical, clicks are
untouched, no value holds a comma, a quote or a newline.

Usage: mutate.py SEED IN_DIR OUT_DIR NAME[,NAME...]
       mutate.py list
NAME is one of the mutations below, `mix` (three to five of them, drawn by the
seed) or `none` (a plain copy).
"""
import datetime
import json
import os
import random
import shutil
import sys

DAY = 86_400_000
GAP = 1_800_000
EPOCH = datetime.datetime(1970, 1, 1)
MASK = (1 << 40) - 1
M1 = 0x9E37_79B9_7F4B
M2 = 0xD6E8_FEB8_6659
M1INV = pow(M1, -1, 1 << 40)
M2INV = pow(M2, -1, 1 << 40)
DIGITS = "0123456789abcdefghijklmnopqrstuvwxyz"
OFFSETS = [-600, -570, -300, -60, 0, 60, 330, 345, 720]
EVENT_HEADER = "event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents"


# ------------------------------------------------------------------ IDs

def base36(value):
    out = ""
    while True:
        out = DIGITS[value % 36] + out
        value //= 36
        if value == 0:
            return out


def scramble(value, key):
    m = value & MASK
    m = (m * M1 ^ key) & MASK
    m ^= m >> 17
    return m * M2 & MASK


def half(number):
    """The scramble undone up to the key: (index * M1) ^ key."""
    m = number * M2INV & MASK
    m ^= m >> 17
    m ^= m >> 34
    return m


def unscramble(number, key):
    return ((half(number) ^ key) * M1INV) & MASK


def number_of(event_id):
    """The number a canonical ID spells (`e`, then lowercase base 36 without a
    leading zero, below 2^40); None for every other ID."""
    s = event_id[1:]
    if not event_id.startswith("e") or not 1 <= len(s) <= 8 or any(c not in DIGITS for c in s):
        return None
    if len(s) > 1 and s[0] == "0":
        return None
    value = int(s, 36)
    return value if value <= MASK else None


def infer_key(numbers, bound):
    """The 40-bit key under which all of `numbers` are indexes below `bound`."""
    if len(numbers) < 6:
        return None
    halves = [half(n) for n in numbers[:8]]
    first, rest = halves[0], halves[1:]
    for index in range(bound):
        key = first ^ (index * M1 & MASK)
        if all(((h ^ key) * M1INV) & MASK < bound for h in rest):
            return key
    return None


# ------------------------------------------------------------------ time

def f1(ms):
    return (EPOCH + datetime.timedelta(milliseconds=ms)).strftime("%Y-%m-%dT%H:%M:%SZ")


def f2(ms, offset_minutes):
    local = EPOCH + datetime.timedelta(milliseconds=ms + offset_minutes * 60_000)
    sign = "-" if offset_minutes < 0 else "+"
    a = abs(offset_minutes)
    return local.strftime("%Y-%m-%dT%H:%M:%S") + f"{sign}{a // 60:02d}:{a % 60:02d}"


def parse_ts(s):
    """Epoch milliseconds of a valid ts (F1, F2 or F3), else None."""
    if s and all(c in "0123456789" for c in s):
        return int(s)
    if len(s) not in (20, 25) or s[4] != "-" or s[7] != "-" or s[10] != "T" or s[13] != ":" or s[16] != ":":
        return None
    digits = s[0:4] + s[5:7] + s[8:10] + s[11:13] + s[14:16] + s[17:19]
    if any(c not in "0123456789" for c in digits):
        return None
    try:
        at = datetime.datetime(int(s[0:4]), int(s[5:7]), int(s[8:10]), int(s[11:13]), int(s[14:16]), int(s[17:19]))
    except ValueError:
        return None
    ms = int((at - EPOCH).total_seconds()) * 1000
    if len(s) == 20:
        return ms if s[19] == "Z" else None
    tail = s[19:]
    if tail[0] not in "+-" or tail[3] != ":" or any(c not in "0123456789" for c in tail[1:3] + tail[4:6]):
        return None
    hours, minutes = int(tail[1:3]), int(tail[4:6])
    if hours > 23 or minutes > 59:
        return None
    offset = (hours * 60 + minutes) * 60_000
    return ms - offset if tail[0] == "+" else ms + offset


# ------------------------------------------------------------------ lines

def split_fields(line, count):
    parts = line.split(",")
    if len(parts) != count:
        return None
    out = []
    for part in parts:
        if '"' in part:
            if len(part) >= 2 and part[0] == '"' and part[-1] == '"' and '"' not in part[1:-1]:
                part = part[1:-1]
            else:
                return None
        out.append(part)
    return out


class Line:
    __slots__ = ("raw", "f", "file", "changed", "gone", "new", "quotes")

    def __init__(self, raw, fields, file):
        self.raw = raw          # the text as read (written back for lines nobody touched)
        self.f = fields         # the seven values, or None for a malformed line
        self.file = file
        self.changed = False    # the values differ from `raw`: written from `f`
        self.gone = False
        self.new = False        # not in the input: goes to a random place in its file
        self.quotes = None      # which fields to quote when written from `f` (bit per field); None = at random


def read_folder(path):
    """[(file name, header line or None, [data lines])], bytes kept exactly (latin-1)."""
    files = []
    for name in sorted(os.listdir(path)) if os.path.isdir(path) else []:
        if not name.endswith(".csv"):
            continue
        with open(os.path.join(path, name), "rb") as f:
            text = f.read().decode("latin-1")
        lines = text.split("\n")
        if lines and lines[-1] == "":
            lines.pop()
        files.append((name, lines[0] if lines else None, lines[1:]))
    return files


class Data:
    def __init__(self, in_dir, seed):
        self.R = random.Random(seed)
        self.in_dir = in_dir
        with open(os.path.join(in_dir, "manifest.json")) as f:
            manifest = json.load(f)
        self.ws = parse_ts(manifest["window_start"])
        self.we = parse_ts(manifest["window_end"])
        self.event_files = read_folder(os.path.join(in_dir, "events"))
        self.link_files = read_folder(os.path.join(in_dir, "links"))
        self.events = []        # every data line of events/, in file order
        for number, (_, _, lines) in enumerate(self.event_files):
            for raw in lines:
                self.events.append(Line(raw, split_fields(raw, 7), number))
        self.links = []         # [file number, raw line]; a removed one is None
        for number, (_, _, lines) in enumerate(self.link_files):
            for raw in lines:
                self.links.append([number, raw])
        self.click_files = read_folder(os.path.join(in_dir, "clicks"))
        self.clicks = None      # read (as Lines of four values) by the mutation that wants them
        self.new_event_files = []   # (name, [field lists])
        self.drop_event_files = False
        self.notes = []
        self.key = None
        self.identity = False
        self.prepare()

    # -- what the mutations need to know about the data

    def prepare(self):
        R = self.R
        # People, by the links as they are now.
        parent = {}

        def find(x):
            while parent.setdefault(x, x) != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x

        for _, raw in self.links:
            f = split_fields(raw, 2)
            if f and f[0] and f[1]:
                parent[find(f[0])] = find(f[1])
        self.person = lambda user: find(user) if user in parent else user
        self.aliases = {}
        for user in list(parent):
            self.aliases.setdefault(find(user), []).append(user)

        # Events by ID, and the index of every canonical ID.
        self.groups = {}
        for line in self.events:
            if line.f and line.f[0]:
                self.groups.setdefault(line.f[0], []).append(line)
        numbers = {eid: number_of(eid) for eid in self.groups}
        canonical = [n for n in numbers.values() if n is not None]
        self.bound = max(64, int(len(self.groups) * 1.5) + 16)
        if self.key is None and canonical:
            # (Once: later, IDs made up by a mutation are among the first ones.)
            self.key = infer_key(canonical, self.bound)
        self.index = {}
        if self.key is not None:
            for eid, n in numbers.items():
                if n is not None:
                    # (After `identityids` an ID spells its index.)
                    v = n if self.identity else unscramble(n, self.key)
                    if v < 4 * self.bound:
                        self.index[eid] = v
        self.top = max(self.index.values(), default=0)
        # Events in index order (generated data), else in the order read; and
        # the runs of consecutive events of one person.
        if self.index:
            order = sorted(self.index, key=self.index.get)
        else:
            order = list(self.groups)
        self.order = order
        self.runs = []
        run, last_person, last_index = [], None, None
        for eid in order:
            who = self.person(self.user_of(eid))
            at = self.index.get(eid)
            follows = last_index is None or at is None or at - last_index <= 2
            if run and (who != last_person or not follows):
                self.runs.append(run)
                run = []
            run.append(eid)
            last_person, last_index = who, at
        if run:
            self.runs.append(run)
        self.where = {eid: (number, at) for number, run in enumerate(self.runs) for at, eid in enumerate(run)}
        self.users = sorted({line.f[1] for line in self.events if line.f and line.f[1]})
        if not self.users:
            self.users = ["u1"]
        R.shuffle(self.users)

    def user_of(self, eid):
        lines = self.groups.get(eid, [])
        return min(lines, key=lambda l: l.f[3]).f[1] if lines else ""

    def time_of(self, eid):
        """The in-window time of an event's kept copy, else None."""
        lines = self.groups.get(eid, [])
        if not lines:
            return None
        ms = parse_ts(min(lines, key=lambda l: l.f[3]).f[2])
        return ms if ms is not None and self.ws <= ms < self.we else None

    # -- building blocks

    def ts_text(self, ms):
        R = self.R
        r = R.random()
        if ms % 1000 or r < 0.3:
            return str(ms)
        if r < 0.4:
            return f2(ms, R.choice(OFFSETS))
        return f1(ms)

    def ingest_after(self, ms, extra_seconds=0):
        return ms - ms % 1000 + (1000 if ms % 1000 else 0) + (self.R.randint(0, 300) + extra_seconds) * 1000

    def set_time(self, eid, ms, user=None):
        """Give every copy of an event the time `ms` (and the user, if given),
        keeping the order of their ingest times and identical copies identical."""
        lines = self.groups.get(eid, [])
        if not lines:
            return
        text = self.ts_text(ms)
        base = self.ingest_after(ms)
        ranks = {old: i for i, old in enumerate(sorted({l.f[3] for l in lines}))}
        for line in lines:
            if parse_ts(line.f[2]) is None:
                continue                      # a bad ts stays bad
            line.f[2] = text
            line.f[3] = f1(base + ranks[line.f[3]] * 1000)
            if user is not None:
                line.f[1] = user
            line.changed = True

    def add_event(self, fields, file=None):
        line = Line(None, list(fields), self.R.randrange(len(self.event_files)) if file is None else file)
        line.changed = True
        line.new = True
        self.events.append(line)
        if line.f[0]:
            self.groups.setdefault(line.f[0], []).append(line)
        return line

    def new_event(self, eid, user=None, ms=None, kind=None):
        R = self.R
        ms = self.ws + R.randrange((self.we - self.ws) // 1000) * 1000 if ms is None else ms
        kind = kind or R.choice(["view", "view", "cart", "purchase"])
        price = str(R.choice([1, 250, 999, 12345])) if kind == "purchase" else R.choice(["", "", "250"])
        return self.add_event([eid, R.choice(self.users) if user is None else user, self.ts_text(ms),
                               f1(self.ingest_after(ms)), kind, "p" + str(R.randint(1, 60000)), price])

    def add_link(self, a, b):
        if not self.link_files:
            return
        self.links.append([self.R.randrange(len(self.link_files)), f"{a},{b}" if self.R.random() < 0.8 else f'"{a}",{b}'])

    def some_runs(self, count, least=2):
        runs = [r for r in self.runs if len(r) >= least]
        self.R.shuffle(runs)
        return runs[:count]

    def id_for(self, index):
        return "e" + base36(index if self.identity else scramble(index, self.key))

    def shift_tail(self, run, start, delta):
        """Move the events of a run from position `start` on by `delta` ms, if
        they all have a time and stay inside the window."""
        times = [self.time_of(e) for e in run[start:]]
        if not times or None in times or max(times) + delta >= self.we or min(times) + delta < self.ws:
            return False
        for eid, when in zip(run[start:], times):
            self.set_time(eid, when + delta)
        return True


# ------------------------------------------------------------------ mutations

def irregular(D):
    """Event IDs that are not `e` + canonical base 36: other IDs by the rules,
    even where they spell the same number."""
    R = D.R
    ids = [e for e in D.groups if len(e) > 1]
    R.shuffle(ids)
    made = 0
    for eid in ids[:max(6, len(ids) // 30)]:
        lines = D.groups[eid]
        kind = R.choice(["upper", "upper_some", "zero", "first", "long", "space", "byte", "empty", "pair", "pair2", "dup"])
        cut = R.randrange(1, len(eid) + 1)
        if kind == "upper":
            new = eid.upper()
        elif kind == "upper_some":
            new = eid[0] + eid[1:].upper()
            lines = [l for l in lines if R.random() < 0.6] or lines[:1]
        elif kind == "zero":
            new = eid[0] + "0" * R.choice([1, 1, 2]) + eid[1:]
        elif kind == "first":
            new = R.choice(["x", "E", "f", "0", "\xc3\xa9"]) + eid[1:]
        elif kind == "long":
            new = eid + "".join(R.choice(DIGITS) for _ in range(R.randint(1, 6)))
        elif kind == "space":
            new = R.choice([eid[:cut] + " " + eid[cut:], eid + " ", " " + eid])
        elif kind == "byte":
            new = eid[:cut] + R.choice(["\xc3\xa9", "_", "-", "A", "\t", "."]) + eid[cut:]
        elif kind == "empty":
            new, lines = "", lines[:1]
        elif kind in ("pair", "pair2"):
            # A second event whose ID spells the same number.
            twin = eid[0] + ("0" if kind == "pair" else "00") + eid[1:]
            when = D.time_of(eid)
            D.new_event(twin, user=R.choice([D.user_of(eid), None]), ms=None if when is None else min(D.we - 1, when + R.choice([0, 1000, GAP, GAP + 1])))
            made += 1
            continue
        else:
            # An irregular ID with copies of its own, some under another user.
            new = eid.upper() if eid.upper() != eid else "E" + eid
            first = lines[0]
            used = {l.f[3] for l in lines}
            for extra in range(R.randint(1, 3)):
                stamp = parse_ts(first.f[3]) + (extra + 1) * R.choice([1, 60, 600]) * 1000
                if f1(stamp) in used:
                    continue
                used.add(f1(stamp))
                copy = list(first.f)
                copy[3] = f1(stamp)
                if R.random() < 0.4:
                    copy[1] = R.choice(D.users)
                lines.append(D.add_event(copy))
        for line in lines:
            line.f[0] = new
            line.changed = True
        made += 1
    D.notes.append(f"irregular {made}")


def outrange(D):
    """Canonical-looking IDs whose index is far outside the data, and the
    largest numbers an ID can spell."""
    R = D.R
    ids = ["ezzzzzzzz", "e" + base36(MASK), "e" + base36(MASK + 1), "e" + base36(MASK + 12345), "e" + base36(MASK - 1),
           "e" + "z" * 7, "e0", "e1", "ez", "e10", "e" + base36(36 ** 8 - 1), "e" + "1" + "0" * 8, "e" + "z" * 9]
    if D.key is not None:
        for index in [D.top + 1, D.top + 2, D.top + 1000, 2 * D.bound, 3 * D.bound + 7, 64 * D.bound, 1 << 26, 1 << 27,
                      (1 << 32) - 1, 1 << 32, 1 << 39, (1 << 39) + 5, MASK, MASK - 1, MASK - 4096]:
            ids.append(D.id_for(index))
    for eid in ids:
        user = R.choice(D.users)
        ms = D.ws + R.randrange((D.we - D.ws) // 1000) * 1000
        for copy in range(R.choice([1, 1, 2, 3])):
            line = D.new_event(eid, user=user if R.random() < 0.8 else None, ms=ms, kind=R.choice(["view", "purchase"]))
            line.f[3] = f1(D.ingest_after(ms, extra_seconds=1000 * (copy + 1)))
    D.notes.append(f"outrange {len(ids)}")


BAD_TS = ["", "x", "2026-13-01T00:00:00Z", "1.7e12", "-5", "2026-01-01 00:00:00Z", "2026-01-01T00:00:00", "2026-01-01T24:00:00Z"]


def copies(D):
    """The same event ID in two to eight copies that disagree about the user,
    the time, validity, lateness, the price."""
    R = D.R
    ids = [e for e in D.groups if D.time_of(e) is not None]
    R.shuffle(ids)
    made = 0
    for eid in ids[:max(8, len(ids) // 25)]:
        lines = D.groups[eid]
        first = lines[0]
        when = D.time_of(eid)
        used = {l.f[3] for l in lines}
        mates = D.aliases.get(D.person(first.f[1]), [first.f[1]])
        for _ in range(R.randint(1, 7)):
            copy = list(first.f)
            if R.random() < 0.12:
                D.add_event(copy)                         # an identical copy, same ingest_ts
                continue
            stamp = parse_ts(first.f[3]) + R.choice([-900, -600, -61, -1, 1, 2, 60, 600, 900, 3600]) * 1000 * R.choice([1, 1, 2])
            r = R.random()
            if r < 0.20:
                copy[1] = R.choice(mates)
            elif r < 0.40:
                copy[1] = R.choice(D.users)
            elif r < 0.45:
                copy[1] = ""
            r = R.random()
            if r < 0.15:
                copy[2] = D.ts_text(max(D.ws, min(D.we - 1, when + R.choice([-3_600_000, -1000, 1000, 1, GAP, GAP + 1, 3_600_000]))))
            elif r < 0.25:
                copy[2] = R.choice(BAD_TS)
            elif r < 0.35:
                copy[2] = D.ts_text(R.choice([D.ws - 1, D.ws - 1000, D.we, D.we + DAY]))
            elif r < 0.45:
                # late, or exactly at the limit: ingested 24 hours (and a second) after ts
                floor = when - when % 1000
                stamp = floor + DAY + R.choice([0, 1000, 2000, 3 * DAY])
            r = R.random()
            if r < 0.25:
                copy[4] = R.choice(["view", "cart", "purchase", "purchase", "Purchase", ""])
            if copy[4] == "purchase" and R.random() < 0.8:
                copy[6] = R.choice(["1", "100", "250", "0250", "10000000", "10000001", "0", "19.99", "", str(R.randint(1, 50000))])
            if copy[4] == "purchase" and R.random() < 0.3:
                copy[5] = "p" + str(R.randint(1, 50000))      # another category, perhaps
            if R.random() < 0.08:
                copy[5] = R.choice(["", "p1", "p999999"])
            if f1(stamp) in used:
                continue
            used.add(f1(stamp))
            copy[3] = f1(stamp)
            line = D.add_event(copy)
            if R.random() < 0.08:
                line.f = None                              # a malformed copy
                line.raw = ",".join(copy[:R.choice([5, 6])]) if R.random() < 0.5 else '"' + ",".join(copy)
        made += 1
    D.notes.append(f"copies {made}")


def disorder(D):
    """Events whose times do not follow their IDs: neighbours swapped, a later
    ID with an earlier time, two users' events interleaved and the users linked."""
    R = D.R
    made = 0
    for run in D.some_runs(max(4, len(D.runs) // 20)):
        i = R.randrange(len(run) - 1)
        a, b = D.time_of(run[i]), D.time_of(run[i + 1])
        if a is None or b is None:
            continue
        if R.random() < 0.5:
            D.set_time(run[i], b)
            D.set_time(run[i + 1], a)
        else:
            D.set_time(run[i + 1], max(D.ws, a - R.randint(1, 10800) * 1000))
        made += 1
    pairs = D.some_runs(12, least=3)
    for a, b in zip(pairs[0::2], pairs[1::2]):
        times = [t for t in (D.time_of(e) for e in a) if t is not None]
        if not times:
            continue
        for eid in b:
            D.set_time(eid, max(D.ws, min(D.we - 1, R.choice(times) + R.randint(-900, 900) * 1000)))
        D.add_link(D.user_of(a[0]), D.user_of(b[0]))
        made += 1
    # A whole run backwards.
    for run in D.some_runs(3, least=4):
        times = [D.time_of(e) for e in run]
        if None in times:
            continue
        for eid, when in zip(run, reversed(times)):
            D.set_time(eid, when)
        made += 1
    D.notes.append(f"disorder {made}")


def gaps(D):
    """Gaps of exactly 30:00.000 and 30:00.001: between neighbours, between the
    last event of one user and the first of a linked one, after shared times."""
    R = D.R
    made = 0
    shift_tail = D.shift_tail

    # Across the places where an index-based pipeline may cut its work: the
    # events just below and at a multiple of a power of two, whoever they
    # belong to (two users are linked into one person first).
    by_index = {v: e for e, v in D.index.items()}
    edges = 0
    for step in (64, 1024, 4096, 8192, 16384, 32768, 65536):
        for edge in range(step, D.top + 1, step):
            if step < 4096 and R.random() > 0.04:
                continue
            a, b = by_index.get(edge - 1), by_index.get(edge)
            if a is None or b is None or a not in D.where or b not in D.where:
                continue
            ta, tb = D.time_of(a), D.time_of(b)
            if ta is None or tb is None:
                continue
            run, at = D.where[b]
            if shift_tail(D.runs[run], at, ta + R.choice([GAP, GAP + 1, GAP, GAP + 1, 0, GAP - 1]) - tb):
                if D.person(D.user_of(a)) != D.person(D.user_of(b)):
                    D.add_link(D.user_of(a), D.user_of(b))
                edges += 1
    made += edges

    for run in D.some_runs(max(6, len(D.runs) // 15)):
        i = R.randrange(len(run) - 1)
        a, b = D.time_of(run[i]), D.time_of(run[i + 1])
        if a is None or b is None:
            continue
        want = a + R.choice([GAP, GAP + 1, GAP - 1, GAP, GAP + 1])
        made += shift_tail(run, i + 1, want - b)
    pairs = D.some_runs(16, least=2)
    for a, b in zip(pairs[0::2], pairs[1::2]):
        last = max((t for t in (D.time_of(e) for e in a) if t is not None), default=None)
        first = D.time_of(b[0])
        if last is None or first is None:
            continue
        if shift_tail(b, 0, last + R.choice([GAP, GAP + 1]) - first):
            D.add_link(D.user_of(a[-1]), D.user_of(b[0]))
            made += 1
    for run in D.some_runs(8, least=4):
        i = R.randrange(len(run) - 3)
        when = D.time_of(run[i])
        tail = D.time_of(run[i + 3])
        if when is None or tail is None:
            continue
        for eid in run[i + 1:i + 3]:
            D.set_time(eid, when)
        made += shift_tail(run, i + 3, when + R.choice([GAP, GAP + 1]) - tail)
    D.notes.append(f"gaps {made} ({edges} at edges)")


def nested(D):
    """Two linked users whose events nest: one user's long stretch (events 25
    minutes apart) with the other's short one inside it, and the other's next
    event within 30 minutes of the END of the long stretch only. Sessions
    built from pieces must be joined by the latest time seen so far, not by the
    end of the piece before."""
    R = D.R
    made = 0
    pairs = D.some_runs(40, least=3)
    for a, b in zip(pairs[0::2], pairs[1::2]):
        if D.person(D.user_of(a[0])) == D.person(D.user_of(b[0])):
            continue
        start = D.ws + R.randrange(max(1, (D.we - D.ws - 6 * 3_600_000) // 1000)) * 1000
        minute = 60_000
        plan_a = [0, 25 * minute, 50 * minute]
        tail = R.choice([75 * minute, 80 * minute, 80 * minute + 1, 50 * minute + GAP, 50 * minute + GAP + 1])
        plan_b = [5 * minute, 6 * minute, tail]
        if R.random() < 0.3:
            plan_b = [0, 0, tail]                       # the same start as the long stretch
        for eid, at in zip(a[:3], plan_a):
            D.set_time(eid, start + at)
        for eid, at in zip(b[:3], plan_b):
            D.set_time(eid, start + at)
        # What follows in either run goes well after, in its old order.
        for run in (a, b):
            first = D.time_of(run[3]) if len(run) > 3 else None
            if first is not None:
                D.shift_tail(run, 3, start + 4 * 3_600_000 - first)
        D.add_link(D.user_of(a[0]), D.user_of(b[0]))
        made += 1
    D.notes.append(f"nested {made}")


def identityids(D):
    """Every canonical event ID replaced by `e` + its index in base 36: with
    the un-scrambling switched off (SS_INDEX_IDENTITY=1) the whole data set then
    goes through the index path, at its own size and shape. Put it first in a
    list of mutations."""
    if D.key is None or D.identity:
        D.notes.append("identityids: no key")
        return
    renamed = 0
    for eid, index in D.index.items():
        new = "e" + base36(index)
        for line in D.groups[eid]:
            line.f[0] = new
            line.changed = True
        renamed += 1
    D.identity = True
    D.notes.append(f"identityids {renamed}")


def links(D):
    """Links between users far apart, and links taken away so that one
    generated user's IDs become two people."""
    R = D.R
    users = D.users
    added = 0
    for _ in range(min(200, max(10, len(users) // 20))):
        D.add_link(R.choice(users), R.choice(users))
        added += 1
    removed = 0
    valid = [i for i, (_, raw) in enumerate(D.links) if (f := split_fields(raw, 2)) and f[0] and f[1] and f[0] != f[1]]
    R.shuffle(valid)
    for at in valid[:max(4, len(valid) // 25)]:
        if D.links[at] is None:
            continue
        pair = set(split_fields(D.links[at][1], 2))
        for other in range(len(D.links)):
            if D.links[other] is not None:
                f = split_fields(D.links[other][1], 2)
                if f and set(f) == pair:
                    D.links[other] = None
                    removed += 1
    D.links = [l for l in D.links if l is not None]
    D.notes.append(f"links +{added} -{removed}")


def wide(D):
    """One person out of fifty users all over the index range, and a user with
    thousands of events in one second."""
    R = D.R
    runs = D.some_runs(50, least=1)
    chain = [D.user_of(run[0]) for run in runs]
    chain = [u for u in chain if u]
    for a, b in zip(chain, chain[1:]):
        D.add_link(a, b)
    longest = max(D.runs, key=len, default=[])
    # A stretch of index-neighbouring events, whoever they belonged to.
    size = min(70000, max(50, len(D.order) // R.choice([3, 4, 10, 40])))
    start = R.randrange(max(1, len(D.order) - size))
    stretch = D.order[start:start + size]
    if len(longest) >= len(stretch):
        stretch = longest[:70000]
    if stretch:
        who = D.user_of(stretch[0]) or R.choice(D.users)
        second = D.ws + R.randrange((D.we - D.ws) // 1000) * 1000
        for eid in stretch:
            D.set_time(eid, second + (R.randrange(1000) if R.random() < 0.3 else 0), user=who)
    D.notes.append(f"wide {len(chain)} linked, {len(stretch)} in one second")


def drop(D):
    """Events removed: single ones, whole index ranges, every other index."""
    R = D.R
    order = D.order
    gone = set(R.sample(order, len(order) // 20)) if order else set()
    for _ in range(3):
        if order:
            start = R.randrange(len(order))
            gone.update(order[start:start + max(1, len(order) // 50)])
    if order:
        start = R.randrange(len(order))
        gone.update(order[start:start + max(2, len(order) // 20):2])
    for eid in gone:
        for line in D.groups.get(eid, []):
            line.gone = True
    D.notes.append(f"drop {len(gone)}")


def irrfile(D):
    """A file whose IDs are all irregular, and a new file of irregular IDs only."""
    R = D.R
    if D.event_files:
        target = R.randrange(len(D.event_files))
        count = 0
        for line in D.events:
            if line.file == target and line.f and line.f[0]:
                line.f[0] = line.f[0].upper() if line.f[0].upper() != line.f[0] else "E" + line.f[0]
                line.changed = True
                count += 1
        D.notes.append(f"irrfile {count} in file {target}")
    rows = []
    for k in range(R.randint(1, 300)):
        ms = D.ws + R.randrange((D.we - D.ws) // 1000) * 1000
        rows.append([R.choice([f"E{k}", f"ev-{k}", f"e0{base36(k)}", f"x{k}", f"e{k} "]), R.choice(D.users), D.ts_text(ms),
                     f1(D.ingest_after(ms)), R.choice(["view", "cart", "purchase"]), "p7", "500"])
    D.new_event_files.append(("part-99990.csv", rows))


def badhead(D):
    """The first lines of the largest events file malformed, irregular, or
    canonical-looking but meaningless: whatever samples the IDs must cope."""
    R = D.R
    if not D.event_files:
        return
    sizes = [0] * len(D.event_files)
    for line in D.events:
        sizes[line.file] += 1
    target = max(range(len(sizes)), key=sizes.__getitem__)
    head = [l for l in D.events if l.file == target][:R.choice([1, 3, 8, 40, 200, 2000])]
    style = R.choice(["malformed", "irregular", "random", "mixed", "same", "same"])
    if style == "same":
        # Copies of one event and nothing else: every sampled ID is the same.
        model = next((l for l in head if l.f and l.f[0]), None)
        if model is None:
            return
        taken = {l.f[3] for l in D.groups.get(model.f[0], [])}
        for line in head:
            if line is model or line.f is None:
                continue
            stamp = parse_ts(model.f[3])
            while f1(stamp) in taken:
                stamp += 1000
            taken.add(f1(stamp))
            line.f = list(model.f)
            line.f[3] = f1(stamp)
            line.changed = True
        D.notes.append(f"badhead {len(head)} same")
        return
    for line in head:
        how = style if style != "mixed" else R.choice(["malformed", "irregular", "random", "keep"])
        if how == "keep" or line.f is None:
            continue
        if how == "malformed":
            text = line.raw if line.raw is not None else ",".join(line.f)
            line.f, line.raw, line.changed = None, R.choice([text + ",x", '"' + text, text.rsplit(",", 1)[0], ""]), False
        elif how == "irregular":
            line.f[0] = R.choice([line.f[0].upper(), line.f[0] + "zzzz", " " + line.f[0], "e0" + line.f[0][1:]])
            line.changed = True
        else:
            line.f[0] = "e" + "".join(R.choice(DIGITS[1:]) for _ in range(R.choice([1, 2, 6, 8, 8, 8])))
            line.f[3] = f1(parse_ts(line.f[3]) + R.randint(-5000, 5000) * 1000)
            line.changed = True
    D.notes.append(f"badhead {len(head)} {style}")


def purchases(D):
    """More purchases, at the edges of sessions too."""
    R = D.R
    ids = list(D.groups)
    R.shuffle(ids)
    for eid in ids[:len(ids) // 8]:
        price = str(R.choice([1, 99, 250, 4999, 10_000_000, R.randint(1, 50000)]))
        product = "p" + str(R.randint(1, 50000))
        for line in D.groups[eid]:
            line.f[4], line.f[5], line.f[6] = "purchase", product, price
            line.changed = True
    D.notes.append(f"purchases {len(ids) // 8}")


def emptyevents(D):
    """Every events file with its header only."""
    for line in D.events:
        line.gone = True
    D.notes.append("emptyevents")


def noeventfiles(D):
    """No file at all in events/."""
    for line in D.events:
        line.gone = True
    D.drop_event_files = True
    D.notes.append("noeventfiles")


F2_OFFSETS = [-1439, -720, -600, -570, -300, -60, -1, 0, 0, 1, 60, 330, 345, 525, 720, 840, 1439]
LENGTHS = [61, 62, 63, 64, 65, 66, 67, 95, 96, 97, 120, 125, 126, 127, 128, 129, 130, 131, 135, 160, 191, 192, 193, 200,
           255, 256, 257, 300, 513, 1000, 4097]


def same_instant(D, text, pad_to=None):
    """Another spelling of a valid ts: F1, F2 at any offset (the local date may
    be another day, even one outside the window), F3 with leading zeros."""
    R = D.R
    ms = parse_ts(text)
    if ms is None or ms < 0 or ms >= 10 ** 15:
        return text
    r = R.random()
    if pad_to:
        return str(ms).zfill(pad_to)
    if ms % 1000 == 0 and r < 0.25:
        return f1(ms)
    if ms % 1000 == 0 and r < 0.60:
        new = f2(ms, R.choice(F2_OFFSETS))
        return new[:19] + "-00:00" if new.endswith("+00:00") and R.random() < 0.5 else new
    if r < 0.80:
        return str(ms)
    return str(ms).zfill(R.choice([13, 14, 15, 16, 17, 18, 19, 20, 21, 24, 25, 26, 30, 40, 64, 100]))


def quote_mask(R, count):
    style = R.choice(["all", "all", "none", "alternate", "random", "first", "last", "outer"])
    full = (1 << count) - 1
    return {"all": full, "none": 0, "alternate": 0x55 & full, "random": R.randrange(full + 1), "first": 1,
            "last": 1 << (count - 1), "outer": 1 | 1 << (count - 1)}[style]


def shapes(D):
    """The same events written in every shape a line can take: quotes around
    all fields or none, each ts as F1, F2 at any offset and F3 with leading
    zeros, prices with leading zeros, event IDs of every length, and lines
    of exactly 63, 64, 65, 127, 128, 129 ... bytes and far longer. Nothing an
    event means changes (an ID is lengthened in all its copies alike), so a
    decoder that works by position has every chance to slip."""
    R = D.R
    # Longer event IDs: zeros after the `e` (no generated ID has one there).
    ids = [e for e in D.groups if len(e) > 1 and e[0] == "e" and e[1] != "0"]
    R.shuffle(ids)
    renamed = 0
    for eid in ids[:len(ids) // 8]:
        new = "e" + "0" * R.choice([1, 1, 1, 2, 2, 3, 8, 20, 60, 130]) + eid[1:]
        for line in D.groups[eid]:
            line.f[0] = new
            line.changed = True
        renamed += 1
    # Copies with equal ingest_ts must stay identical: they change together.
    clusters = {}
    for line in D.events:
        if line.f and line.f[0]:
            clusters.setdefault((line.f[0], line.f[3]), []).append(line)
    keys = list(clusters)
    R.shuffle(keys)
    done = 0
    for key in keys[:max(60, len(keys) // 3)]:
        lines = clusters[key]
        f = list(lines[0].f)
        if any(l.f != f for l in lines):
            continue
        purchase = f[4] == "purchase"
        f[2] = same_instant(D, f[2])
        if purchase:
            if f[6] and all(c in "0123456789" for c in f[6]) and R.random() < 0.5:
                f[6] = "0" * R.choice([1, 2, 3, 8, 13, 20, 40]) + f[6]
        else:
            if R.random() < 0.5:
                f[6] = R.choice(["", "0", "7", "00000000000000000000001", "abc", "-1", "9" * 30, "1.5", " ", "x" * 70])
        mask = quote_mask(R, 7)
        if R.random() < 0.6:
            # A line of an exact length: the padding goes where it means nothing.
            length = sum(len(v) for v in f) + 6 + 2 * bin(mask).count("1")
            target = R.choice([t for t in LENGTHS if t > length] or [length])
            pad = target - length
            if purchase:
                if f[6] and all(c in "0123456789" for c in f[6]):
                    f[6] = "0" * pad + f[6]
            elif f[5]:
                f[5] = f[5] + "x" * pad
        for line in lines:
            line.f = list(f)
            line.changed = True
            line.quotes = mask if R.random() < 0.8 else quote_mask(R, 7)
        done += 1
    D.notes.append(f"shapes {done} lines, {renamed} ids lengthened")


def clickshapes(D):
    """Click lines in every shape: quotes, each ts as F1, F2 and F3 with
    leading zeros (the same in all copies of a click), exact line lengths."""
    R = D.R
    if D.clicks is None:
        D.clicks = [Line(raw, split_fields(raw, 4), number)
                    for number, (_, _, lines) in enumerate(D.click_files) for raw in lines]
    groups = {}
    for line in D.clicks:
        if line.f and line.f[0]:
            groups.setdefault(line.f[0], []).append(line)
    ids = list(groups)
    R.shuffle(ids)
    done = 0
    for cid in ids[:max(40, len(ids) // 3)]:
        lines = groups[cid]
        f = list(lines[0].f)
        if any(l.f != f for l in lines):
            continue
        mask = quote_mask(R, 4)
        f[3] = same_instant(D, f[3])
        if R.random() < 0.4 and parse_ts(f[3]) is not None:
            length = sum(len(v) for v in f[:3]) + 3 + 2 * bin(mask).count("1")
            target = R.choice([t for t in LENGTHS if t > length + 13] or [length + 13])
            f[3] = same_instant(D, f[3], pad_to=target - length)
        for line in lines:
            line.f = list(f)
            line.changed = True
            line.quotes = mask if R.random() < 0.8 else quote_mask(R, 4)
        done += 1
    D.notes.append(f"clickshapes {done}")


def linkshapes(D):
    """Link lines quoted in every way, and links between new IDs of every length."""
    R = D.R
    done = 0
    for entry in D.links:
        f = split_fields(entry[1], 2)
        if f and R.random() < 0.3:
            mask = R.randrange(4)
            entry[1] = ",".join('"' + v + '"' if mask >> i & 1 else v for i, v in enumerate(f))
            done += 1
    if D.link_files:
        for k in range(R.randint(5, 60)):
            a = "u" + "".join(R.choice(DIGITS) for _ in range(R.choice([1, 2, 7, 8, 9, 10, 15, 16, 17, 31, 32, 33, 62, 70, 130])))
            b = R.choice([a + "z", "u" + base36(k) + "q" * R.choice([1, 8, 9, 40]), R.choice(D.users)]) if R.random() < 0.9 else a
            D.links.append([R.randrange(len(D.link_files)), ",".join('"' + v + '"' if R.random() < 0.4 else v for v in (a, b))])
    D.notes.append(f"linkshapes {done}")


MUTATIONS = {f.__name__: f for f in [irregular, outrange, copies, disorder, gaps, nested, links, wide, drop, irrfile, badhead,
                                     purchases, shapes, clickshapes, linkshapes, identityids, emptyevents, noeventfiles]}
MIXABLE = ["irregular", "outrange", "copies", "disorder", "gaps", "nested", "links", "wide", "drop", "irrfile", "badhead",
           "purchases", "shapes", "shapes", "clickshapes", "linkshapes"]


# ------------------------------------------------------------------ writing

def encode(D, fields, quotes=None):
    if quotes is not None:
        return ",".join('"' + v + '"' if quotes >> i & 1 else v for i, v in enumerate(fields))
    return ",".join('"' + v + '"' if D.R.random() < 0.2 else v for v in fields)


def write(D, out_dir):
    R = D.R
    os.makedirs(os.path.join(out_dir, "events"), exist_ok=True)
    per_file = [[] for _ in D.event_files]
    added = [[] for _ in D.event_files]
    for line in D.events:
        if not line.gone:
            (added if line.new else per_file)[line.file].append(line)   # a line of the input keeps its place
    for number, extra in enumerate(added):
        for line in extra:
            per_file[number].insert(R.randrange(len(per_file[number]) + 1), line)
    # What the rules promise: copies with equal ingest_ts are identical. A line
    # that would break it (two mutations met on one event) is left out.
    seen = {}
    texts = []
    for number, lines in enumerate(per_file):
        out = []
        for line in lines:
            if line.f is None:
                out.append(line.raw)
                continue
            text = encode(D, line.f, line.quotes) if line.changed or line.raw is None else line.raw
            if line.f[0]:
                key = (line.f[0], line.f[3])
                if seen.setdefault(key, line.f) != line.f:
                    continue
            out.append(text)
        texts.append(out)
    if not D.drop_event_files:
        for (name, header, _), lines in zip(D.event_files, texts):
            with open(os.path.join(out_dir, "events", name), "wb") as f:
                f.write(((header or EVENT_HEADER) + "\n" + "".join(l + "\n" for l in lines)).encode("latin-1"))
        for name, rows in D.new_event_files:
            lines = []
            for row in rows:
                key = (row[0], row[3])
                if row[0] and seen.setdefault(key, row) != row:
                    continue
                lines.append(encode(D, row))
            with open(os.path.join(out_dir, "events", name), "wb") as f:
                f.write((EVENT_HEADER + "\n" + "".join(l + "\n" for l in lines)).encode("latin-1"))
    os.makedirs(os.path.join(out_dir, "links"), exist_ok=True)
    link_lines = [[] for _ in D.link_files]
    for number, raw in D.links:
        link_lines[number].append(raw)
    for (name, header, _), lines in zip(D.link_files, link_lines):
        with open(os.path.join(out_dir, "links", name), "wb") as f:
            f.write(((header or "user_id_a,user_id_b") + "\n" + "".join(l + "\n" for l in lines)).encode("latin-1"))
    if D.clicks is None:
        shutil.copytree(os.path.join(D.in_dir, "clicks"), os.path.join(out_dir, "clicks"))
    else:
        os.makedirs(os.path.join(out_dir, "clicks"), exist_ok=True)
        click_lines = [[] for _ in D.click_files]
        for line in D.clicks:
            click_lines[line.file].append(encode(D, line.f, line.quotes) if line.f is not None and line.changed else line.raw)
        for (name, header, _), lines in zip(D.click_files, click_lines):
            with open(os.path.join(out_dir, "clicks", name), "wb") as f:
                f.write(((header or "click_id,user_id,campaign_id,ts") + "\n" + "".join(l + "\n" for l in lines)).encode("latin-1"))
    for name in ("products.csv", "campaigns.csv", "manifest.json"):
        shutil.copy(os.path.join(D.in_dir, name), os.path.join(out_dir, name))


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "list":
        print(" ".join(MUTATIONS))
        return
    seed, in_dir, out_dir, names = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
    if os.path.exists(out_dir):
        shutil.rmtree(out_dir)
    D = Data(in_dir, seed)
    chosen = []
    for name in names.split(","):
        if name == "mix":
            chosen += D.R.sample(MIXABLE, D.R.randint(3, 5))
        elif name != "none":
            chosen.append(name)
    for name in chosen:
        MUTATIONS[name](D)
        # The next mutation sees the data as it is now.
        D.events = [l for l in D.events if not l.gone]
        D.prepare()
    write(D, out_dir)
    key = "none" if D.key is None else f"{D.key:010x}"
    print(f"mutate seed {seed} [{','.join(chosen) or 'none'}]: key {key}, {len(D.groups)} ids, top index {D.top}; " + "; ".join(D.notes))


if __name__ == "__main__":
    main()
