#!/usr/bin/env python3
"""
Mutations of a generated data set: input on which a pipeline's short cuts can
go wrong while the rules (rules.md) stay perfectly well defined. Lines in
every shape and length a splitter or a decoder that works by position has to
cope with, copies of events that disagree, IDs of every kind (for version 5 of
the generator, whose IDs are opaque and one in eight is a letter and 32 hex
digits: that long form and everything near it, for events, users and clicks),
times on the edges of sessions and of lateness, links added and removed.

For data of the generator's earlier versions (IDs are `e` + base 36 of a
40-bit bijection of a dense index that a pipeline could decode) the event key
is inferred and events are taken in index order; otherwise a person's events
are taken in time order.

The output keeps what the rules promise about the data: ingest_ts is a valid
F1 timestamp, copies of an event with equal ingest_ts are identical, copies of
a click are identical, no value holds a comma, a quote or a newline.

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

def up(text):
    """Upper case for the ASCII letters only: a text here is bytes read as
    latin-1, and str.upper() would turn a byte like 0xFF into a character that
    is no byte at all."""
    return "".join(chr(ord(c) - 32) if "a" <= c <= "z" else c for c in text)


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
    __slots__ = ("raw", "f", "file", "changed", "gone", "new", "quotes", "members", "held")

    def __init__(self, raw, fields, file):
        self.members = None     # a block: texts and Lines written one after the other, in this line's place
        self.held = False       # written as a member of a block, not on its own
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
        self.endings = False
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
            # No index to go by (version 5 of the generator: IDs are opaque):
            # a person's events together, in time order, the ones without a
            # time last.
            def place_of(eid):
                when = self.time_of(eid)
                return (self.person(self.user_of(eid)), when is None, when or 0, eid)
            order = sorted(self.groups, key=place_of)
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

    def load_clicks(self):
        """The click lines as Lines of four values (None for a malformed one)."""
        if self.clicks is None:
            self.clicks = [Line(raw, split_fields(raw, 4), number)
                           for number, (_, _, lines) in enumerate(self.click_files) for raw in lines]
        return self.clicks

    def campaigns(self):
        """The campaign IDs of campaigns.csv."""
        with open(os.path.join(self.in_dir, "campaigns.csv"), "rb") as f:
            lines = f.read().decode("latin-1").split("\n")[1:]
        return [line.split(",")[0].strip('"') for line in lines if line] or ["c_1"]

    def all_users(self):
        """Every user ID in events, clicks and links."""
        taken = set(self.users)
        for line in self.load_clicks():
            if line.f:
                taken.add(line.f[1])
        for entry in self.links:
            f = split_fields(entry[1], 2)
            if f:
                taken.update(f)
        return taken

    def rename_users(self, mapping):
        """The users of `mapping` under their new names, in events, clicks and links."""
        R = self.R
        for line in self.events:
            if line.f and line.f[1] in mapping:
                line.f[1] = mapping[line.f[1]]
                line.changed = True
        for line in self.load_clicks():
            if line.f and line.f[1] in mapping:
                line.f[1] = mapping[line.f[1]]
                line.changed = True
        for entry in self.links:
            f = split_fields(entry[1], 2)
            if f and (f[0] in mapping or f[1] in mapping):
                f = [mapping.get(v, v) for v in f]
                entry[1] = ",".join('"' + v + '"' if R.random() < 0.3 else v for v in f)

    def link(self, a, b):
        """A link line, either side quoted or not, in a random file."""
        if self.link_files:
            R = self.R
            self.links.append([R.randrange(len(self.link_files)), ",".join('"' + v + '"' if R.random() < 0.3 else v for v in (a, b))])

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
            new = up(eid)
        elif kind == "upper_some":
            new = eid[0] + up(eid[1:])
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
            new = up(eid) if up(eid) != eid else "E" + eid
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


def idbytes(D):
    """Event IDs one byte away from canonical ones, for a decoder that turns
    the bytes after `e` into a number several lines at a time. Each is a twin:
    a new event next to the original (same user or another, a time near it),
    so a decoder that takes the twin for the original loses an event. A digit
    replaced by a byte next to the digit ranges, by what a decoder without
    range checks would read as the same digit, by itself with a bit flipped;
    the ID with NULs, a space or a zero after it, with another first byte;
    IDs that differ from each other in NULs at the end only (one hash, if the
    hash pads with zeros)."""
    R = D.R
    near = ["/", ":", "`", "{", "@", "[", "\x00", " ", "\x7f", "\x80", "\xff", "G", "Z", "_", "\r", "\t", "\xb1"]
    ids = [e for e in D.groups if 2 <= len(e) <= 9 and D.time_of(e) is not None]
    R.shuffle(ids)
    made = 0
    for eid in ids[:max(60, len(ids) // 12)]:
        kind = R.choice(["near", "near", "alias", "alias", "flip", "tail", "head", "nuls", "zero", "all"])
        at = R.randrange(1, len(eid))
        c = eid[at]
        twins = []
        if kind == "alias" and c not in DIGITS:
            kind = "near"
        if kind == "near":
            twins = [eid[:at] + R.choice(near) + eid[at + 1:]]
        elif kind == "alias":
            v = int(c, 36)
            # `:` to `S` continue the digits; `W` to `` ` `` sit before `a` as 0 to 9 do before 10.
            other = chr(ord("0") + v) if c >= "a" else chr(ord("a") - 10 + v)
            twins = [eid[:at] + R.choice([other, up(c) if c >= "a" else other]) + eid[at + 1:]]
        elif kind == "flip":
            twins = [eid[:at] + chr(ord(c) ^ R.choice([0x80, 0x20, 0x10, 0x08, 0x40, 0x01])) + eid[at + 1:]]
        elif kind == "tail":
            twins = [eid + t for t in R.sample(["\x00", "\x00\x00", " ", "0", "\x000", "\xff", "\x80"], 2)]
        elif kind == "head":
            twins = [h + eid[1:] for h in R.sample(["E", "d", "f", "\xe5", "%", "\x00", "u", "k", "1"], 2)]
        elif kind == "nuls":
            base = up(eid) if up(eid) != eid else "E" + eid[1:]
            twins = [base, base + "\x00", base + "\x00\x00", base + "\x00\x00\x00"][:R.randint(2, 4)]
        elif kind == "zero":
            twins = [eid[0] + "0" + eid[2:], eid[0] + "0" + eid[1:], eid[:at] + "0" * (len(eid) - at)]
        else:
            odd = R.choice(near)
            twins = [eid[0] + odd * (len(eid) - 1), eid[0] + odd * 8, eid[0] + odd]
        when = D.time_of(eid)
        user = D.user_of(eid)
        for twin in twins:
            if not twin or len(twin) > 12 or any(ch in twin for ch in ',"\n') or twin in D.groups:
                continue
            # MUTATE_SKIP=nultail leaves out IDs of at most 9 bytes that end in
            # a NUL: a known defect (such an ID is taken for the ID without the
            # NULs) hides whatever else a data set would show.
            if "nultail" in os.environ.get("MUTATE_SKIP", "") and len(twin) <= 9 and twin.endswith("\x00"):
                continue
            ms = min(D.we - 1, when + R.choice([0, 1000, 60_000, GAP, GAP + 1]))
            D.new_event(twin, user=R.choice([user, user, None]), ms=ms, kind=R.choice(["view", "cart", "purchase"]))
            made += 1
    D.notes.append(f"idbytes {made}")


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
                line.f[0] = up(line.f[0]) if up(line.f[0]) != line.f[0] else "E" + line.f[0]
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
            line.f[0] = R.choice([up(line.f[0]), line.f[0] + "zzzz", " " + line.f[0], "e0" + line.f[0][1:]])
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
        # One field at a length drawn evenly from a wide range (a decoder with
        # fixed places for the fields has a limit somewhere).
        if R.random() < 0.5:
            want = R.randint(0, 40)
            if purchase:
                if f[6] and all(c in "0123456789" for c in f[6]):
                    f[6] = f[6].lstrip("0").zfill(max(want, 1)) if f[6].strip("0") else f[6]
            elif R.random() < 0.5 and f[5]:
                f[5] = "p" + "".join(R.choice(DIGITS) for _ in range(want)) if R.random() < 0.8 else "\xc3\xa9" * (1 + want // 2)
            else:
                f[6] = R.choice(["7", "x", "0", " ", "\x00", "\xc3\xa9"]) * want
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


def userlengths(D):
    """Users renamed, everywhere alike (events, clicks, links), to IDs of every
    length and kind: one or two characters, up to fifty, zeros after the `u`,
    upper case, a space, a multi-byte character, a NUL byte. The people stay
    the same people under other names."""
    R = D.R
    if D.clicks is None:
        D.clicks = [Line(raw, split_fields(raw, 4), number)
                    for number, (_, _, lines) in enumerate(D.click_files) for raw in lines]
    taken = set(D.users)
    for line in D.clicks:
        if line.f:
            taken.add(line.f[1])
    for entry in D.links:
        f = split_fields(entry[1], 2)
        if f:
            taken.update(f)
    chosen = R.sample(D.users, min(len(D.users), max(20, len(D.users) // 8)))
    mapping = {}
    for k, user in enumerate(chosen):
        style = R.choice(["long", "long", "long", "short", "zeros", "utf8", "space", "upper", "nul", "other"])
        cut = R.randrange(1, len(user) + 1)
        if style == "long":
            new = user + "".join(R.choice(DIGITS) for _ in range(R.randint(1, 42)))
        elif style == "short":
            new = "u" + base36(k)[:R.choice([0, 1, 1, 2, 3])]
        elif style == "zeros":
            new = user[0] + "0" * R.randint(1, 12) + user[1:]
        elif style == "utf8":
            new = user[:cut] + R.choice(["\xc3\xa9", "\xe2\x82\xac", "\xf0\x9f\x98\x80"]) + user[cut:]
        elif style == "space":
            new = R.choice([user + " ", " " + user, user[:cut] + " " + user[cut:], user + "\t"])
        elif style == "upper":
            new = up(user)
        elif style == "nul":
            new = user[:cut] + "\x00" + user[cut:]
        else:
            new = R.choice(["x", "U", "0", "user_id", "-"]) + user[1:]
        if not new or new in taken:
            continue
        taken.add(new)
        mapping[user] = new
    for line in D.events:
        if line.f and line.f[1] in mapping:
            line.f[1] = mapping[line.f[1]]
            line.changed = True
    for line in D.clicks:
        if line.f and line.f[1] in mapping:
            line.f[1] = mapping[line.f[1]]
            line.changed = True
    for entry in D.links:
        f = split_fields(entry[1], 2)
        if f and (f[0] in mapping or f[1] in mapping):
            f = [mapping.get(v, v) for v in f]
            entry[1] = ",".join('"' + v + '"' if R.random() < 0.3 else v for v in f)
    D.notes.append(f"userlengths {len(mapping)}")


def lateness(D):
    """Events on the 24-hour edge between ts and ingest_ts, in every ts form.
    With an offset the date a ts shows is not its UTC date: a ts written as
    the day of its ingest_ts can be more than 24 hours before it, and the
    other way round. With milliseconds, 24 hours and a second after the whole
    second is still late, 24 hours is not."""
    R = D.R
    singles = [e for e, lines in D.groups.items() if len(lines) == 1 and D.time_of(e) is not None]
    R.shuffle(singles)
    made = 0
    for eid in singles[:max(40, len(singles) // 10)]:
        line = D.groups[eid][0]
        when = D.time_of(eid)
        when -= when % 1000
        style = R.randrange(6)
        if style == 0:
            # Late, and the local date of ts is the UTC date of ingest_ts.
            off = R.choice([60, 330, 345, 525, 720, 840, 1439])
            day = when - when % DAY
            earliest = DAY - off * 60_000                     # the local date is the next day from here on
            t = day + R.randrange(earliest // 1000, DAY // 1000) * 1000
            room = (day + 2 * DAY - (t + DAY)) // 1000        # seconds left of the day ingest_ts must be on
            if not D.ws <= t < D.we or room < 2:
                continue
            line.f[2] = f2(t, off)
            line.f[3] = f1(t + DAY + R.randrange(1, room) * 1000)
        elif style == 1:
            # Not late, though the dates the two show are two days apart.
            off = R.choice([-1439, -720, -600, -300, -60])
            day = when - when % DAY
            t = day + R.randrange(0, -off * 60) * 1000        # early in the UTC day: the local date is the day before
            if not D.ws <= t < D.we:
                continue
            line.f[2] = f2(t, off)
            line.f[3] = f1(day + DAY + R.randrange(0, max(1, (t - day) // 1000 + 1)) * 1000)
        elif style == 2:
            # Milliseconds: 24 h after the whole second is kept, a second more is late.
            ms = R.randrange(1, 1000)
            line.f[2] = str(when + ms) if R.random() < 0.8 else str(when + ms).zfill(R.choice([14, 16, 17, 20, 25]))
            line.f[3] = f1(when + DAY + R.choice([0, 1000, 1000, -1000, 2000]))
        else:
            # Exactly 24 hours, a second more, a second less, in any form.
            delta = R.choice([DAY, DAY + 1000, DAY - 1000, DAY, DAY + 1000, 2 * DAY, DAY + 3_600_000, 0, -5000])
            r = R.random()
            line.f[2] = f1(when) if r < 0.3 else f2(when, R.choice(F2_OFFSETS)) if r < 0.7 else str(when).zfill(R.choice([13, 16, 20, 25]))
            line.f[3] = f1(when + delta)
        line.changed = True
        if R.random() < 0.5:
            line.quotes = quote_mask(R, 7)
        made += 1
    D.notes.append(f"lateness {made}")


NEAR_TYPES = ["viewcart", "cartview", "view\x00", "cart\x00\x00\x00\x00", "purchasex", "xpurchase", "purchase\x00", "vie", "viewx",
              "car", "p", "purchas", "PURCHASE", "View", " view", "view ", "", "viewview", "purchaseview", "purchase ", "cartx",
              "vieW", "wiev", "trac", "purchasf", "qurchase", "view\xc3\xa9", "cart\t"]
NEAR_PRICES = ["0", "00", "1", "10000000", "10000001", "9999999", "010000000", "0000000010000000", "00000000100000001",
               "99999999999999999", "18446744073709551617", "1e3", "+5", "5 ", " 5", "5.0", "-1", "", "\xd9\xa5", "12a", "0x10",
               "1_000", "100000000", "0000000000000000000000000000000000000001"]


def values(D):
    """Near misses of the values the rules name: event types one byte off or
    padded, prices at both ends of their range and beyond a 64-bit integer,
    products that are empty or not in the catalogue. (These events change
    what they mean; the yardstick is the plain implementation.)"""
    R = D.R
    clusters = {}
    for line in D.events:
        if line.f and line.f[0]:
            clusters.setdefault((line.f[0], line.f[3]), []).append(line)
    keys = list(clusters)
    R.shuffle(keys)
    made = 0
    for key in keys[:max(60, len(keys) // 12)]:
        lines = clusters[key]
        f = list(lines[0].f)
        if any(l.f != f for l in lines):
            continue
        r = R.random()
        if r < 0.35:
            f[4] = R.choice(NEAR_TYPES)
        elif r < 0.75:
            f[4] = "purchase"
            f[6] = R.choice(NEAR_PRICES)
            if R.random() < 0.3:
                f[5] = R.choice(["", "p0", "p50001", "P1", "p1 ", "p01", "p", "p4194304", "p99999999", "q1", "p1\x00"])
        else:
            f[5] = R.choice(["", "p0", "p50001", "P1", "p1 ", "p01", "p", "p4194304", "p99999999999", "q1"])
        mask = quote_mask(R, 7) if R.random() < 0.5 else None
        for line in lines:
            line.f = list(f)
            line.changed = True
            line.quotes = mask
        made += 1
    D.notes.append(f"values {made}")


def stretches(D):
    """Stretches of 70 to 400 consecutive lines of one file that all need the
    same rare treatment: all purchases, all with ingest_ts on the UTC day
    after ts, all with a user ID no shortcut takes, all with a long event ID.
    (A parser that works in batches keeps a short list of the lines with more
    to do; here it is every line of the batch.)"""
    R = D.R
    by_file = {}
    for line in D.events:
        if not line.new:
            by_file.setdefault(line.file, []).append(line)
    files = [lines for lines in by_file.values() if len(lines) >= 80]
    R.shuffle(files)
    clusters = {}
    for line in D.events:
        if line.f and line.f[0]:
            clusters.setdefault((line.f[0], line.f[3]), []).append(line)
    made = []
    for lines in files[:6]:
        size = min(len(lines), R.choice([70, 128, 129, 200, 257, 400]))
        start = R.randrange(len(lines) - size + 1)
        style = R.choice(["purchase", "midnight", "both", "user", "longid", "quoted", "longest", "longest"])
        seen = set()
        for line in lines[start:start + size]:
            if line.f is None or not line.f[0] or (line.f[0], line.f[3]) in seen:
                continue
            f = list(line.f)
            key = (f[0], f[3])
            if any(l.f != f for l in clusters.get(key, [])):
                continue
            when = parse_ts(f[2])
            if style in ("purchase", "both"):
                f[4] = "purchase"
                f[5] = f[5] or "p1"
                f[6] = str(R.choice([1, 250, 4999, 10_000_000, R.randint(1, 50000)]))
            if style in ("midnight", "both") and when is not None and D.ws <= when < D.we:
                # The last seconds of a UTC day; ingested in the first seconds of the next.
                day_end = when - when % DAY + DAY
                t = day_end - R.randint(1, 30) * 1000
                if D.ws <= t < D.we:
                    f[2] = D.ts_text(t)
                    stamp = day_end + R.randint(0, 600) * 1000
                    while (f[0], f1(stamp)) in clusters or (f[0], f1(stamp)) in seen:
                        stamp += 1000
                    f[3] = f1(stamp)
            if style == "user" and f[1]:
                f[1] = R.choice([f[1] + "0123456789", up(f[1]), " " + f[1], f[1] + "\xc3\xa9"])
            if style == "longid":
                f[0] = f[0] + "".join(R.choice(DIGITS) for _ in range(R.randint(2, 12)))
            seen.add((f[0], f[3]))
            mask = 127 if style == "quoted" else None
            if style == "longest":
                # As long as a line can be for a splitter with 64-byte windows
                # (third comma at the last byte of a window, the first one the
                # line's IDs leave room for; newline 64 bytes on, or 128), and
                # now and then a byte more on either side.
                mask = quote_mask(R, 7)
                head = len(f[0]) + len(f[1]) + 15 + 2 * bin(mask & 7).count("1")
                comma = next((m - 1 for m in (64, 128, 192, 256, 320) if m - 1 >= head), None)
                if comma is not None:
                    place(f, mask, comma + R.choice([0, 0, 0, 0, -1, 1]), R.choice([64, 64, 64, 64, 63, 65, 128, 127, 129]))
            for twin in clusters.get(key, [line]):
                twin.f = list(f)
                twin.changed = True
                twin.quotes = mask
        made.append(f"{style} {size}")
    D.notes.append("stretches " + ", ".join(made))


def place(f, mask, comma3, newline_after):
    """Put a line's third comma at byte `comma3` and its newline
    `newline_after` bytes after that comma, by spellings that change nothing:
    zeros in front of an F3 ts (any valid ts becomes one), zeros in front of
    a purchase's price, any tail on a product nobody looks up. (Where that
    cannot be done, the line stays shorter.)"""
    q = [mask >> i & 1 for i in range(7)]
    ms = parse_ts(f[2])
    if comma3 is not None and ms is not None and 0 <= ms < 10 ** 14:
        # Before the third comma: the first three fields, their quotes, two commas.
        other = len(f[0]) + len(f[1]) + 2 + 2 * (q[0] + q[1] + q[2])
        if comma3 - other >= len(str(ms)):
            f[2] = str(ms).zfill(comma3 - other)
    if newline_after is not None:
        # After it: the last four fields, their quotes, three commas, and one.
        rest = len(f[3]) + len(f[4]) + len(f[5]) + len(f[6]) + 3 + 2 * (q[3] + q[4] + q[5] + q[6]) + 1
        if newline_after > rest:
            if f[4] == "purchase":
                if f[6] and all(c in "0123456789" for c in f[6]):
                    f[6] = "0" * (newline_after - rest) + f[6]
            elif f[5]:
                f[5] = f[5] + "y" * (newline_after - rest)


def near_mark(R, least):
    """A byte count next to a multiple of 64 (three below it to three above),
    not below `least`: mostly next to the first multiple there is room for,
    else next to any up to 320. None if `least` is beyond them all."""
    options = [m + d for m in (64, 128, 192, 256, 320) for d in (-3, -2, -1, 0, 1, 2, 3) if m + d >= least]
    if not options:
        return None
    return R.choice(options[:7]) if R.random() < 0.6 else R.choice(options)


def splitter(D):
    """For a splitter that works on 64-byte blocks, eight lines at a step:
    the third comma of a line next to a multiple of 64 (the first one the
    line's IDs leave room for, or a later one), the newline as many bytes
    after it, quotes on either side of those marks; runs of empty and of one-
    to seven-byte lines (more than eight line ends inside one block) among
    ordinary lines; lines with no comma, five, seven; lines of 9,000 to 20,000
    bytes followed by ordinary ones; a lone quote for a field, `""` for every
    field; runs of lines with an empty event_id, quoted or not, long enough to
    cross every batch boundary."""
    R = D.R
    clusters = {}
    for line in D.events:
        if line.f and line.f[0]:
            clusters.setdefault((line.f[0], line.f[3]), []).append(line)
    keys = list(clusters)
    R.shuffle(keys)
    placed = 0
    for key in keys[:max(80, len(keys) // 4)]:
        lines = clusters[key]
        f = list(lines[0].f)
        if any(l.f != f for l in lines):
            continue
        mask = quote_mask(R, 7)
        # What the line needs at least before its third comma (a ts of 13
        # digits) and from there to its newline.
        head = len(f[0]) + len(f[1]) + 15 + 2 * bin(mask & 7).count("1")
        tail = len(f[3]) + len(f[4]) + len(f[5]) + len(f[6]) + 5 + 2 * bin(mask >> 3).count("1")
        place(f, mask, near_mark(R, head), near_mark(R, tail))
        for line in lines:
            line.f = list(f)
            line.changed = True
            line.quotes = mask
        placed += 1
    # A few lines far longer than anything a batch scans in one go.
    long_ones = 0
    for key in keys[-12:]:
        lines = clusters[key]
        f = list(lines[0].f)
        if any(l.f != f for l in lines) or f[4] == "purchase" or not f[5]:
            continue
        f[5] = f[5] + "z" * R.randint(9000, 20000)
        for line in lines:
            line.f = list(f)
            line.changed = True
        long_ones += 1

    model = next((l.f for l in D.events if l.f and l.f[0] and parse_ts(l.f[2]) is not None), None)
    if model is None or not D.event_files:
        D.notes.append(f"splitter {placed} placed")
        return

    model = list(model)

    def block(members):
        """Lines that stay together, at a random place in a random file."""
        line = Line(None, None, R.randrange(len(D.event_files)))
        line.new = True
        line.members = members
        D.events.append(line)

    def empty_id():
        f = list(model)
        f[0] = ""
        f[1] = R.choice(D.users)
        text = ",".join(f)
        return R.choice([text, '""' + text, text, '""' + text, '"",' + ",".join('"' + v + '"' for v in f[1:])])

    def ordinary():
        """A copy of a line of the data (the same values, the same ingest_ts:
        an identical copy by the rules). It is a line like any other for the
        mutations that follow, so the copies stay identical."""
        source = R.choice(D.events)
        if source.f is None or source.members is not None:
            return None
        copy = Line(source.raw, list(source.f), source.file)
        copy.changed, copy.quotes, copy.held = source.changed, source.quotes, True
        D.events.append(copy)
        return copy

    shorts = ["", "", "", "a", "ab", "abc", "abcd", "abcde", "abcdef", "abcdefg", ",", ",,", '"', '""', ",,,,,,", "1,2,3",
              ",,,,,", ",,,,,,,", '"",', '""""', "\r", " "]
    runs = 0
    for _ in range(R.randint(6, 14)):
        kind = R.choice(["short", "short", "empty", "mixed", "emptyid", "emptyid", "commas", "quotes", "long", "sevens", "sevens"])
        size = R.choice([1, 2, 7, 8, 9, 15, 16, 17, 31, 33, 63, 64, 65, 70, 130, 300])
        texts = []
        if kind == "short":
            texts = [R.choice(shorts) for _ in range(size)]
        elif kind == "sevens":
            # Seven empty fields in seven bytes: the shortest line that has the
            # right shape (and no event_id). Nine of them end inside one block.
            for _ in range(size):
                r = R.random()
                texts.append(",,,,,," if r < 0.7 else '"",,,,,,' if r < 0.8 else ',,,,,,""' if r < 0.9 else (ordinary() or ",,,,,,"))
        elif kind == "empty":
            texts = [""] * size
        elif kind == "mixed":
            for _ in range(size):
                texts.append(R.choice(shorts) if R.random() < 0.7 else (ordinary() or ""))
        elif kind == "emptyid":
            for _ in range(size):
                texts.append(empty_id() if R.random() < 0.8 else (ordinary() or empty_id()))
        elif kind == "commas":
            body = ",".join(model)
            for _ in range(size):
                # (Cut before the last comma: never seven fields.)
                texts.append(R.choice(["x" * R.randint(1, 200), body.rsplit(",", 1)[0], body + ",x", body + ",", "," + body,
                                       body.replace(",", ";"), body.replace(",", ",,", 1), body[:R.randrange(body.rfind(","))]]))
        elif kind == "quotes":
            parts = list(model)
            filled = [k for k in range(7) if parts[k]]
            for _ in range(size):
                at = R.choice(filled)                     # (a field with something in it: every variant is malformed)
                texts.append(R.choice([
                    ",".join(parts[:at] + ['"'] + parts[at + 1:]),                    # a lone quote for a field
                    ",".join(['""'] * 7),                                             # nothing but quoted nothing
                    ",".join(parts[:at] + ['"' + parts[at]] + parts[at + 1:]),        # opened, never closed
                    ",".join(parts[:at] + [parts[at] + '"'] + parts[at + 1:]),        # closed, never opened
                    ",".join(parts[:at] + ['"' + parts[at] + '""'] + parts[at + 1:]),
                    '"' + ",".join(parts) + '"',                                      # quotes at both ends of the line
                    ",".join(parts[:at] + ['""' + parts[at]] + parts[at + 1:]),
                ]))
        else:
            texts = [R.choice(["x" * R.randint(9000, 20000), ",".join(model) + "w" * R.randint(9000, 20000) + '"',
                               "," * R.randint(9000, 12000)])]
            texts += [ordinary() or "" for _ in range(R.randint(1, 20))]
        # Copies of ordinary lines are fine (identical, same ingest_ts); the
        # rest is malformed or has no event_id.
        block(texts)
        runs += 1
    D.notes.append(f"splitter {placed} placed, {long_ones} long, {runs} runs")


def clicksplit(D):
    """The same for click lines (four fields, a splitter that takes lines whose
    newline is within 64 bytes): lines of 60 to 66 bytes and of 126 to 129, a
    few of thousands of bytes (zeros in front of an F3 ts change nothing);
    runs of empty lines, of `,,,` (four empty fields in three bytes: sixteen
    line ends in one block, all of the right shape), of lines without click_id
    or without user_id, quoted and not; lines with 0, 2 and 4 commas; a lone
    quote for a field; all among copies of real lines."""
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
    placed = 0
    for at, cid in enumerate(ids[:max(60, len(ids) // 3)]):
        lines = groups[cid]
        f = list(lines[0].f)
        ms = parse_ts(f[3])
        if any(l.f != f for l in lines) or ms is None or not 0 <= ms < 10 ** 14:
            continue
        mask = quote_mask(R, 4)
        # (No more than 4,000 digits: Python's own limit for a number's text is
        # 4,300, and the reference reads a ts with int(). Longer lines are made
        # of long IDs below.)
        other = len(f[0]) + len(f[1]) + len(f[2]) + 3 + 2 * bin(mask).count("1")
        want = (near_mark(R, other + 13) or 0) if at >= 3 else R.randint(1500, 4000)
        if want - other >= len(str(ms)):
            f[3] = str(ms).zfill(want - other)
        for line in lines:
            line.f = list(f)
            line.changed = True
            line.quotes = mask
        placed += 1
    model = next((list(l.f) for l in D.clicks if l.f and l.f[0] and l.f[1] and parse_ts(l.f[3]) is not None), None)
    if model is None or not D.click_files:
        D.notes.append(f"clicksplit {placed} placed")
        return
    fresh = [0]

    def new_id():
        fresh[0] += 1
        return "kZ" + base36(fresh[0]) + "".join(R.choice(DIGITS) for _ in range(R.randint(0, 6)))

    def ordinary():
        source = R.choice(D.clicks)
        if source.f is None or source.members is not None:
            return None
        copy = Line(source.raw, list(source.f), source.file)
        copy.changed, copy.quotes, copy.held = source.changed, source.quotes, True
        D.clicks.append(copy)
        return copy

    body = ",".join(model)
    # (One text per click_id: copies of a click are identical.)
    shorts = ["", "", "", ",,,", ",,,", '"",,,', ',"",,', ',,,""', "a,b,c,d", '"a",b,c,d', ",", ",,", ",,,,", "a", "ab", "abcdefg",
              '"', '""', "\r", " ", ",,,x", "x,,,"]
    runs = 0
    for _ in range(R.randint(6, 14)):
        kind = R.choice(["short", "short", "empty", "fours", "fours", "noclick", "nouser", "commas", "quotes", "long", "mixed"])
        size = R.choice([1, 2, 7, 8, 9, 15, 16, 17, 31, 33, 63, 64, 65, 70, 130, 300])
        members = []
        for _ in range(size):
            if kind == "short":
                members.append(R.choice(shorts))
            elif kind == "empty":
                members.append("")
            elif kind == "fours":
                members.append(R.choice([",,,", ",,,", ",,,", '"",,,', ',"",,', ',,,""', '"","","",""']) if R.random() < 0.9 else (ordinary() or ",,,"))
            elif kind == "noclick":
                rest = ",".join(model[1:])
                members.append(R.choice(["," + rest, '"",' + rest, '"",' + ",".join('"' + v + '"' for v in model[1:])]) if R.random() < 0.8 else (ordinary() or ""))
            elif kind == "nouser":
                members.append(R.choice([f"{new_id()},,{model[2]},{model[3]}", f'{new_id()},"",{model[2]},{model[3]}',
                                         f'"{new_id()}","","{model[2]}","{model[3]}"']) if R.random() < 0.8 else (ordinary() or ""))
            elif kind == "commas":
                members.append(R.choice(["x" * R.randint(1, 120), body.rsplit(",", 1)[0], body.rsplit(",", 2)[0], body + ",x", body + ",",
                                         "," + body, body.replace(",", ";"), body[:R.randrange(body.rfind(","))]]))
            elif kind == "quotes":
                k = R.randrange(4)
                members.append(R.choice([",".join(model[:k] + ['"'] + model[k + 1:]), ",".join(model[:k] + ['"' + model[k]] + model[k + 1:]),
                                         ",".join(model[:k] + [model[k] + '"'] + model[k + 1:]), '"' + body + '"',
                                         ",".join(model[:k] + ['"' + model[k] + '""'] + model[k + 1:])]))
            elif kind == "long":
                size = R.randint(9000, 20000)
                # Not a click at all, or a new click that is long by its
                # click_id, by its user (whom nobody knows) or by its campaign.
                members.append(R.choice(["x" * size, body + "9" * size + '"', "," * R.randint(9000, 12000),
                                         f"{new_id()}{'7' * size},{model[1]},{model[2]},{model[3]}",
                                         f"{new_id()},u{'8' * size},{model[2]},{model[3]}",
                                         f"{new_id()},{model[1]},{model[2]}{'x' * size},{model[3]}"]))
                members += [ordinary() or "" for _ in range(R.randint(1, 20))]
                break
            else:
                members.append(R.choice(shorts) if R.random() < 0.6 else (ordinary() or ""))
        block = Line(None, None, 0)
        block.members = members
        at = R.randrange(len(D.clicks))
        block.file = D.clicks[at].file
        D.clicks.insert(at + 1, block)
        runs += 1
    D.notes.append(f"clicksplit {placed} placed, {runs} runs")


def interleave(D):
    """In a few files, after every line (or every second, third, seventh,
    eighth, ninth) a line that is not an event: empty, seven empty fields,
    right shape without an event_id, a few bytes, one field too many. A
    splitter that takes lines eight at a time then meets its odd lines at
    every phase, for thousands of lines on end."""
    R = D.R
    model = next((list(l.f) for l in D.events if l.f and l.f[0] and parse_ts(l.f[2]) is not None), None)
    if model is None:
        D.notes.append("interleave: nothing to copy")
        return
    sizes = {}
    for line in D.events:
        sizes[line.file] = sizes.get(line.file, 0) + 1
    # The largest files (most of the lines are there), and a few of the others.
    files = sorted(sizes, key=sizes.get, reverse=True)
    small = files[3:]
    R.shuffle(small)
    plan = {}
    for number in files[:R.randint(1, 3)] + small[:R.randint(1, 3)]:
        plan[number] = (R.choice([1, 1, 2, 3, 7, 8, 9]), R.choice(["empty", "seven", "emptyid", "emptyid", "short", "extra", "any"]))
    body = ",".join([""] + model[1:])
    count = {number: 0 for number in plan}
    made = 0
    out = []
    for line in D.events:
        out.append(line)
        if line.file not in plan or line.new or line.held:
            continue
        every, kind = plan[line.file]
        count[line.file] += 1
        if count[line.file] % every:
            continue
        how = kind if kind != "any" else R.choice(["empty", "seven", "emptyid", "short", "extra"])
        text = {"empty": "", "seven": ",,,,,,", "emptyid": R.choice([body, '""' + body]),
                "short": R.choice(["a", "ab", "a,b", "abcdefg", '"', ",,"]), "extra": ",".join(model) + ",x"}[how]
        out.append(Line(text, None, line.file))
        made += 1
    D.events = out
    D.notes.append(f"interleave {made} lines: " + ", ".join(f"file {n} every {e} {k}" for n, (e, k) in plan.items()))


def endings(D):
    """Files that end without a newline after their last line, or with an
    empty line (a malformed line by the rules), in all three folders."""
    D.endings = True
    D.notes.append("endings")


# ------------------------------------------------------------------ long IDs
#
# Version 5 of the generator writes one distinct ID in eight (events, users,
# clicks) as its prefix letter and 32 lowercase hex digits: 33 bytes. The
# rules know no such form: IDs are opaque strings that compare byte by byte.
# A pipeline with a short cut for the 33-byte form must take exactly that
# form by it and nothing that looks like it.

HEX = "0123456789abcdef"


def is_long(value, prefix=None):
    return len(value) == 33 and (prefix is None or value[0] == prefix) and all(c in HEX for c in value[1:])


def sayable(value):
    """No comma, quote or newline: the rules promise that of every value."""
    return bool(value) and not any(c in value for c in ',"\n')


class Longs:
    """IDs of the long form and their near misses. A hex part now and then
    shares its first 8, 16, 24 or 31 digits with one made before, or its last
    16 or 31, or all but one, or has its two halves the other way round: a
    table that hashes a part of the ID has its collisions."""

    def __init__(self, R, seeds=()):
        self.R = R
        self.made = list(seeds)[:40]

    def hex(self):
        R = self.R
        fresh = "".join(R.choice(HEX) for _ in range(32))
        r = R.random()
        if r < 0.03:
            fresh = R.choice(["0" * 32, "f" * 32, "0" * 31 + "1", "0" * 16 + "f" * 16, "f" * 16 + "0" * 16, "1" + "0" * 31,
                              "8" + "0" * 31, "7" + "f" * 31, "0" * 15 + "1" + "0" * 16])
        elif r < 0.08:
            # One half of the digits all zeros (a table of 128-bit values may
            # keep zero for "empty"), or all but a digit.
            fresh = R.choice(["0" * 16 + fresh[16:], fresh[:16] + "0" * 16, "0" * 15 + fresh[15:], fresh[:17] + "0" * 15,
                              "0" * 16 + fresh[16:], fresh[:16] + "0" * 16])
        elif self.made and r < 0.45:
            base = R.choice(self.made)
            keep = R.choice(["first8", "first16", "first16", "first24", "first31", "last16", "last16", "last31", "one", "swap"])
            if keep.startswith("first"):
                n = int(keep[5:])
                fresh = base[:n] + fresh[n:]
            elif keep.startswith("last"):
                n = int(keep[4:])
                fresh = fresh[:32 - n] + base[32 - n:]
            elif keep == "one":
                at = R.randrange(32)
                fresh = base[:at] + R.choice(HEX) + base[at + 1:]
            else:
                fresh = base[16:] + base[:16]
        self.made.append(fresh)
        if len(self.made) > 60:
            del self.made[R.randrange(30)]
        return fresh

    def near(self, prefix, s=None, text=False):
        """An ID one step from the long form `prefix + s` that is not that ID:
        32 or 34 bytes, a last character that is no hex digit, upper case,
        another first letter, a NUL, a byte that is not ASCII (`text`: valid
        UTF-8 only, for IDs that reach the output), a space, the ID's
        beginning. (The caller checks that it is new.)"""
        R = self.R
        s = s or self.hex()
        at = R.randrange(32)
        kind = R.choice(["len32", "len32", "len34", "len34", "last", "last", "upper", "upper", "upper1", "prefix", "prefix",
                         "prefixup", "nul", "nul", "utf8", "high", "odd", "flip", "space", "short", "short", "zero"])
        if kind == "high" and text:
            kind = "utf8"
        if kind == "len32":
            return prefix + R.choice([s[:31], s[1:]])
        if kind == "len34":
            return prefix + R.choice([s + R.choice(HEX), R.choice(HEX) + s, s + "0"])
        if kind == "last":
            return prefix + s[:31] + R.choice("gAF GZz:/`@")
        if kind == "upper":
            return prefix + up(s)
        if kind == "upper1":
            letters = [i for i, c in enumerate(s) if c in "abcdef"]
            if not letters:
                return prefix + s[:31] + "A"
            i = R.choice(letters)
            return prefix + s[:i] + up(s[i]) + s[i + 1:]
        if kind == "prefix":
            return R.choice([c for c in "euk" if c != prefix] + ["x", "0", "g", "\x00"]) + s
        if kind == "prefixup":
            return up(prefix) + s
        if kind == "nul":
            return prefix + s[:at] + "\x00" + s[at + 1:]
        if kind == "utf8":
            at = min(at, 30)
            return prefix + s[:at] + "\xc3\xa9" + s[at + 2:]            # still 33 bytes
        if kind == "high":
            return prefix + s[:at] + R.choice(["\x80", "\xff", "\xe9", chr(ord(s[at]) | 0x80)]) + s[at + 1:]
        if kind == "odd":
            return prefix + s[:at] + R.choice("gG:/`@ xz-_.") + s[at + 1:]
        if kind == "flip":
            return prefix + s[:at] + chr(ord(s[at]) ^ R.choice([0x20, 0x40, 0x10])) + s[at + 1:]
        if kind == "space":
            return R.choice([prefix + s + " ", " " + prefix + s, prefix + " " + s, prefix + s + "\t"])
        if kind == "short":
            return prefix + s[:R.choice([1, 2, 7, 8, 9, 15, 16, 17, 24])]
        return prefix + R.choice(["0" + s, s[:16], s[16:], s[:16] + s[:16]])


def relatives(L, value):
    """IDs that sort right next to `value`, on either side: its beginning, the
    ID and more, another case, another last character, the same first 8, 16
    or 31 digits. (Which of two IDs is the smaller, byte by byte, names a
    person and settles a tie between clicks.)"""
    R = L.R
    out = []
    if len(value) > 2:
        out.append(value[:R.randrange(2, len(value))])
        out.append(value[:-1])
    out.append(value + R.choice(["0", "a", "f", "\x00", " ", "0" * 8]))
    out.append(value[0] + up(value[1:]))
    out.append(up(value[0]) + value[1:])
    out.append(value[:-1] + R.choice(HEX + "gz"))
    letters = [i for i, c in enumerate(value) if i and "a" <= c <= "z"]
    if letters:
        i = R.choice(letters)
        out.append(value[:i] + up(value[i]) + value[i + 1:])
    if is_long(value):
        fresh = "".join(R.choice(HEX) for _ in range(32))
        out += [value[:9] + fresh[8:], value[:17] + fresh[16:], value[:17] + fresh[16:], value[:32] + R.choice(HEX),
                value[0] + fresh[:16] + value[17:], value[:R.choice([2, 8, 9, 10, 17])],
                # A short ID that is the long one's first eight digits, or next to them.
                value[:9], value[:9], value[:8] + R.choice(HEX + "gz"), value[:8] + chr(ord(value[8]) + 1),
                value[:8] + chr(ord(value[8]) - 1), value[:9] + "0"]
    else:
        out += [value[0] + L.hex(), (value + L.hex())[:33], value + L.hex()]
    return [v for v in out if v != value and sayable(v)]


def broken(R, value):
    """A field that holds `value` with a quote where no quote may be: the
    whole line is malformed by the rules. (A reader of standard CSV takes a
    doubled quote inside a quoted field for a quote in the value.)"""
    at = R.randrange(1, len(value)) if len(value) > 1 else 0
    return R.choice([
        '"' + value[:at] + '""' + value[at:] + '"',     # a doubled quote inside quotes
        '"' + value + '""',                              # ... at the end
        '""' + value + '"',                              # ... at the start
        value[:-1] + '"',                                # a quote for the last character
        value + '"', '"' + value,                        # closed, never opened; opened, never closed
        value[:at] + '"' + value[at:],                   # a quote inside
        '"' + value[:at] + '"' + value[at:] + '"',       # quoted, with a quote inside
        value[:at] + '""' + value[at:],                  # a doubled quote without quotes around
    ])


def longusers(D):
    """Users under IDs of the long form and its near misses, everywhere alike
    (events, clicks, both sides of links): the same people under other names.
    Then people with more names than one: a long ID linked to a short one,
    either way round and to itself; the beginning of an ID (which sorts before
    it), the ID and more (after it), the ID in upper case (before every lower-
    case one), linked to it, so that the smallest, which the output reports,
    is now a long one and now a short one; some of the new names with rows of
    their own."""
    R = D.R
    L = Longs(R, [u[1:] for u in D.users if is_long(u, "u")])
    taken = D.all_users()
    # A fifth of the users; now and then all of them (most IDs long: a table
    # sized for the usual share of long IDs runs full).
    share = R.choice([5, 5, 5, 5, 2, 1])
    chosen = R.sample(D.users, min(len(D.users), max(40, len(D.users) // share)))
    mapping = {}
    before = None
    # Now and then a family: tens or hundreds of users whose long IDs have the
    # same first 16 digits, or the same last 16, or differ in one digit.
    family = R.choice([0, 0, 50, 200, 600])
    stem = L.hex()
    style = R.choice(["first16", "first16", "last16", "one", "first24"])
    for number, user in enumerate(chosen):
        r = R.random()
        if number < family:
            fresh = "".join(R.choice(HEX) for _ in range(32))
            at = R.randrange(32)
            new = "u" + {"first16": stem[:16] + fresh[16:], "last16": fresh[:16] + stem[16:], "first24": stem[:24] + fresh[24:],
                         "one": stem[:at] + R.choice(HEX) + stem[at + 1:]}[style]
        elif before is None or r < 0.35:
            new = "u" + L.hex()
        elif r < 0.60:
            new = R.choice(relatives(L, before))
        else:
            new = L.near("u", text=True)
        if not sayable(new) or new in taken:
            continue
        taken.add(new)
        mapping[user] = new
        if is_long(new, "u"):
            before = new
    D.rename_users(mapping)
    users = [mapping.get(u, u) for u in D.users]
    aliases = 0
    for name in R.sample(users, min(len(users), max(40, len(users) // 8))):
        for _ in range(R.choice([1, 1, 2, 3])):
            r = R.random()
            if r < 0.25:
                alias = "u" + L.hex()
            elif r < 0.75:
                alias = R.choice(relatives(L, name))
            elif is_long(name, "u"):
                alias = L.near("u", name[1:], text=True)
            else:
                alias = L.near("u", text=True)
            if not sayable(alias) or alias == name:
                continue
            if R.random() < 0.5:
                D.link(alias, name)
            else:
                D.link(name, alias)
            if R.random() < 0.2:
                D.link(alias, alias)
            if R.random() < 0.15:
                D.link(name, alias)
            if R.random() < 0.3 and alias not in taken:
                D.new_event("e" + L.hex(), user=alias)
            taken.add(alias)
            aliases += 1
    # Now and then thousands of long IDs that no row uses, linked in pairs, in
    # chains, to people who have rows: a table for long IDs fills up with them.
    flood = R.choice([0, 0, 0, 500, 5000, 30000]) if D.link_files else 0
    last = None
    for _ in range(flood):
        fresh = "u" + "".join(R.choice(HEX) for _ in range(32))
        r = R.random()
        D.links.append([R.randrange(len(D.link_files)), f"{fresh},{last}" if last and r < 0.5 else
                        f"{R.choice(users)},{fresh}" if r < 0.6 else f"{fresh},u" + "".join(R.choice(HEX) for _ in range(32))])
        last = fresh
    # Link lines that are malformed by a quote in a long ID: they join nobody.
    # (Taken as links they would join two people who both have rows.)
    bad = 0
    if D.link_files and len(users) > 1:
        for _ in range(R.randint(10, 60)):
            a, b = R.sample(users, 2)
            if is_long(a, "u") or is_long(b, "u") or R.random() < 0.3:
                sides = [broken(R, a), b] if R.random() < 0.5 else [a, broken(R, b)]
                D.links.append([R.randrange(len(D.link_files)), ",".join(sides)])
                bad += 1
    D.notes.append(f"longusers {len(mapping)} renamed ({min(family, len(chosen))} in a family), {aliases} aliases, "
                   f"{flood} unused, {bad} malformed links")


def longevents(D):
    """Event IDs of the long form and its near misses. Events renamed (all
    copies alike); twins of long IDs, each a new event one step away from an
    existing one (32 or 34 bytes, one character that is no lowercase hex
    digit, another case, another first letter, the same first 8, 16 or 31
    digits), so that a pipeline which takes the twin for the original loses an
    event; families of hundreds of IDs with the same first 16 digits, the same
    last 16, or one digit apart; copies of long-ID events under another user,
    and an earlier copy with a bad ts that takes the event away."""
    R = D.R
    L = Longs(R, [e[1:] for e in list(D.groups)[:400] if is_long(e, "e")])
    ids = [e for e in D.groups if e]
    R.shuffle(ids)
    names = set(D.groups)
    lines_of = dict(D.groups)
    renamed = 0
    for eid in ids[:max(40, len(ids) // 10)]:
        new = "e" + L.hex() if R.random() < 0.5 else L.near("e")
        if not sayable(new) or new in names:
            continue
        names.add(new)
        for line in lines_of[eid]:
            line.f[0] = new
            line.changed = True
        lines_of[new] = lines_of.pop(eid)
        renamed += 1

    def first_of(eid):
        return min(lines_of[eid], key=lambda l: l.f[3])

    def time_of(eid):
        ms = parse_ts(first_of(eid).f[2])
        return ms if ms is not None and D.ws <= ms < D.we else None

    def add(eid, user, ms, kind=None):
        names.add(eid)
        return D.new_event(eid, user=user, ms=ms, kind=kind)

    longs = [e for e in lines_of if is_long(e, "e") and time_of(e) is not None]
    R.shuffle(longs)
    twins = 0
    for eid in longs[:max(60, len(longs) // 5)]:
        when, user = time_of(eid), first_of(eid).f[1]
        options = relatives(L, eid) + [L.near("e", eid[1:]) for _ in range(4)]
        for twin in R.sample(options, R.choice([1, 1, 2, 3])):
            if not sayable(twin) or twin in names:
                continue
            add(twin, R.choice([user, user, None]), min(D.we - 1, when + R.choice([0, 1000, 60_000, GAP, GAP + 1])),
                R.choice(["view", "cart", "purchase"]))
            twins += 1
    families = 0
    for _ in range(R.randint(1, 3)):
        stem = L.hex()
        style = R.choice(["first16", "first16", "last16", "first31", "one", "first8"])
        users = R.sample(D.users, min(len(D.users), R.choice([1, 3, 40])))
        start = D.ws + R.randrange(max(1, (D.we - D.ws) // 1000 - 7200)) * 1000
        for k in range(R.choice([20, 64, 150, 400])):
            fresh = "".join(R.choice(HEX) for _ in range(32))
            if style == "first16":
                new = stem[:16] + fresh[16:]
            elif style == "last16":
                new = fresh[:16] + stem[16:]
            elif style == "first31":
                new = stem[:31] + HEX[k % 16] if k < 16 else stem[:30] + fresh[30:]
            elif style == "one":
                at = R.randrange(32)
                new = stem[:at] + R.choice(HEX) + stem[at + 1:]
            else:
                new = stem[:8] + fresh[8:]
            new = "e" + new
            if new in names:
                continue
            add(new, R.choice(users), min(D.we - 1, start + R.choice([k * 1000, k * 60_000, R.randrange(7_200_000)])))
        families += 1
    copies_made = 0
    for eid in longs[:max(30, len(longs) // 10)]:
        first = first_of(eid)
        used = {l.f[3] for l in lines_of[eid]}
        for _ in range(R.choice([1, 1, 2])):
            copy = list(first.f)
            r = R.random()
            if r < 0.15:
                D.add_event(copy)                                   # an identical copy
                continue
            stamp = parse_ts(first.f[3]) + R.choice([-600, -1, 1, 60, 600]) * 1000
            if f1(stamp) in used:
                continue
            used.add(f1(stamp))
            copy[3] = f1(stamp)
            if r < 0.55:
                copy[1] = R.choice(D.users)                         # another user: counts only if it is the earlier copy
            elif r < 0.85:
                copy[2] = R.choice(BAD_TS)                          # earlier and bad: the event is gone
            D.add_event(copy)
            copies_made += 1
    # Lines that are malformed by a quote in a long event ID: an earlier copy
    # of a real event, which would take its place (under another user) if it
    # were read; or a line of its own, which would be one event more.
    bad = 0
    for eid in longs[:max(30, len(longs) // 10)]:
        first = first_of(eid)
        copy = list(first.f)
        copy[1] = R.choice(D.users)
        copy[3] = f1(parse_ts(first.f[3]) - R.choice([1, 60, 3600]) * 1000)
        copy[0] = broken(R, eid if R.random() < 0.6 else "e" + L.hex())
        line = D.add_event(copy)
        line.f = None
        line.raw = ",".join(copy)
        bad += 1
    D.notes.append(f"longevents {renamed} renamed, {twins} twins, {families} families, {copies_made} copies, {bad} malformed")


def longclicks(D):
    """Click IDs of the long form and its near misses (all copies of a click
    alike), and ties: at the very millisecond of an existing click, the same
    person's click for another campaign, under an ID that sorts right next to
    the first one's: long against long (same first 8, 16, 31 digits), long
    against its own beginning, short against short, a change of case. The
    smallest click_id, byte by byte, takes the session. Some ties are put at
    the exact time of an event, where a session may start."""
    R = D.R
    clicks = D.load_clicks()
    campaigns = D.campaigns()
    groups = {}
    for line in clicks:
        if line.f and line.f[0]:
            groups.setdefault(line.f[0], []).append(line)
    L = Longs(R, [c[1:] for c in list(groups)[:400] if is_long(c, "k")])
    ids = list(groups)
    R.shuffle(ids)
    names = set(ids)
    renamed = 0
    for cid in ids[:max(40, len(ids) // 8)]:
        new = "k" + L.hex() if R.random() < 0.5 else L.near("k")
        if not sayable(new) or new in names:
            continue
        names.add(new)
        for line in groups[cid]:
            line.f[0] = new
            line.changed = True
        groups[new] = groups.pop(cid)
        renamed += 1
    if not D.click_files:
        D.notes.append("longclicks: no click files")
        return

    def add(cid, user, campaign, ts):
        names.add(cid)
        line = Line(None, [cid, user, campaign, ts], R.randrange(len(D.click_files)))
        line.changed = True
        clicks.insert(R.randrange(len(clicks) + 1), line)

    def mate(user):
        """The user, or another ID of the same person."""
        return R.choice(D.aliases.get(D.person(user), [user]) + [user, user])

    valid = [c for c, lines in groups.items() if lines[0].f[1] and parse_ts(lines[0].f[3]) is not None
             and all(l.f == lines[0].f for l in lines)]
    R.shuffle(valid)
    ties = 0
    for cid in valid[:max(80, len(valid) // 8)]:
        f = groups[cid][0].f
        options = relatives(L, cid)
        for other in R.sample(options, min(len(options), R.choice([1, 1, 2, 3]))):
            if other in names:
                continue
            add(other, mate(f[1]), R.choice(campaigns), same_instant(D, f[3]))
            ties += 1
    starts = 0
    for run in D.some_runs(max(40, len(D.runs) // 10), least=1):
        eid = R.choice(run)
        when, user = D.time_of(eid), D.user_of(eid)
        if when is None or not user:
            continue
        base = R.choice(["k" + L.hex(), "k" + L.hex(), "k" + "".join(R.choice(DIGITS) for _ in range(R.randint(1, 8)))])
        family = [base] + R.sample(relatives(L, base), R.choice([1, 2, 3]))
        for cid in family:
            if cid in names:
                continue
            add(cid, mate(user), R.choice(campaigns), R.choice([str(when), same_instant(D, str(when))]))
            starts += 1
            if R.random() < 0.3:
                # The same click once more (identical values, as the rules
                # promise; other quotes, another place in the files).
                source = next(l for l in clicks if l.f and l.f[0] == cid)
                copy = Line(None, list(source.f), R.randrange(len(D.click_files)))
                copy.changed = True
                clicks.insert(R.randrange(len(clicks) + 1), copy)
    # Ties between IDs of the lengths at either end: one byte, hundreds of
    # bytes, and more bytes than fit a 14-bit length (16,383), where the two
    # IDs differ in their last byte only or one is the other's beginning.
    extremes = 0
    for run in D.some_runs(12, least=1):
        eid = R.choice(run)
        when, user = D.time_of(eid), D.user_of(eid)
        if when is None or not user:
            continue
        size = R.choice([300, 300, 1000, 16382, 16383, 16384, 17000])
        stem = "k" + "".join(R.choice(DIGITS) for _ in range(size - 2))
        family = R.choice([["k", "k0", "j", "l"], ["0", "k", "K"], [stem + "a", stem + "b"], [stem + "b", stem + "a", stem],
                           [stem + "a", stem + "aa"], ["k" + L.hex(), "k"], [stem + "z", stem[:300], stem[:9]]])
        for cid in family:
            if cid in names:
                continue
            add(cid, mate(user), R.choice(campaigns), str(when))
            extremes += 1
    # Lines that are malformed by a quote in a long click ID, at the time of a
    # real click and for another campaign: read as clicks, they would be in
    # the tie (a quote sorts before every letter and digit).
    bad = 0
    for cid in valid[:max(30, len(valid) // 20)]:
        f = groups[cid][0].f
        text = ",".join([broken(R, cid if is_long(cid, "k") and R.random() < 0.6 else "k" + L.hex()), f[1], R.choice(campaigns), f[3]])
        clicks.insert(R.randrange(len(clicks) + 1), Line(text, None, R.randrange(len(D.click_files))))
        bad += 1
    D.notes.append(f"longclicks {renamed} renamed, {ties} ties, {starts} at event times, {extremes} of extreme length, {bad} malformed")


def windows(D):
    """For splitters that see a line through three 64-byte windows: the ID and
    its comma in the first; from that comma on, an event's user, ts and their
    commas, or a click's user and its comma, in the second; the rest, up to
    the newline, in the third. Fields that end at a window's edge, a few bytes
    before it and after it: IDs of 58 to 66 bytes (events and clicks renamed,
    all copies alike; users renamed everywhere), a ts padded with zeros so
    that an event's third comma comes 60 to 67 bytes after its first and the
    newline as many after the third, a click's newline 60 to 67 bytes after
    its second comma; under every pattern of quotes (a 33-byte ID in quotes
    is 35). After such a line now and then a short one or two, whose newline
    comes inside a window the line before filled."""
    R = D.R
    edge = [60, 61, 62, 62, 63, 63, 64, 64, 65, 65, 66, 67]

    def padded(value, taken):
        """`value` and more, 58 to 66 bytes in all (None: too long, or taken)."""
        want = R.choice([58, 59, 60, 61, 62, 62, 63, 63, 64, 64, 65, 66])
        if len(value) + 1 > want:
            return None
        new = value + "_" + "".join(R.choice(DIGITS) for _ in range(want - len(value) - 1))
        return None if new in taken else new

    # Users of 58 to 66 bytes: a click's second window, an event's second
    # window overfull.
    taken = D.all_users()
    mapping = {}
    for user in R.sample(D.users, min(len(D.users), max(20, len(D.users) // 30))):
        new = padded(user, taken)
        if new:
            taken.add(new)
            mapping[user] = new
    D.rename_users(mapping)

    # Event IDs of 58 to 66 bytes, all copies alike.
    names = set(D.groups)
    ids = [e for e in D.groups if e]
    R.shuffle(ids)
    long_ids = 0
    for eid in ids[:max(40, len(ids) // 7)]:
        new = padded(eid, names)
        if new:
            names.add(new)
            for line in D.groups[eid]:
                line.f[0] = new
                line.changed = True
            long_ids += 1

    # Event lines: the third comma 60 to 67 bytes after the first, the newline
    # as many after the third.
    clusters = {}
    for line in D.events:
        if line.f and line.f[0]:
            clusters.setdefault((line.f[0], line.f[3]), []).append(line)
    keys = list(clusters)
    R.shuffle(keys)
    touched = set()
    placed = 0
    for key in keys[:max(80, len(keys) // 3)]:
        lines = clusters[key]
        f = list(lines[0].f)
        if any(l.f != f for l in lines):
            continue
        mask = quote_mask(R, 7)
        first = len(f[0]) + 2 * (mask & 1)
        place(f, mask, first + R.choice(edge) if R.random() < 0.8 else None, R.choice(edge) if R.random() < 0.8 else None)
        for line in lines:
            line.f = list(f)
            line.changed = True
            line.quotes = mask
            touched.add(id(line))
        placed += 1
    shorts = ["", "a", "ab", ",,,,,,", '"",,,,,,', "x,y", ",", "abcdefghij", ",,,,,,,"]
    out = []
    after = 0
    for line in D.events:
        out.append(line)
        if id(line) in touched and not line.new and not line.held and R.random() < 0.15:
            for _ in range(R.choice([1, 1, 2, 3])):
                out.append(Line(R.choice(shorts), None, line.file))
                after += 1
    D.events = out

    # Clicks: IDs of 58 to 66 bytes (all copies alike), and the newline 60 to
    # 67 bytes after the second comma.
    clicks = D.load_clicks()
    groups = {}
    for line in clicks:
        if line.f and line.f[0]:
            groups.setdefault(line.f[0], []).append(line)
    click_names = set(groups)
    cids = list(groups)
    R.shuffle(cids)
    click_placed = 0
    touched = set()
    for cid in cids[:max(60, len(cids) // 3)]:
        lines = groups[cid]
        f = list(lines[0].f)
        ms = parse_ts(f[3])
        if any(l.f != f for l in lines) or ms is None or not 0 <= ms < 10 ** 14:
            continue
        if R.random() < 0.4:
            new = padded(cid, click_names)
            if new:
                click_names.add(new)
                f[0] = new
        mask = quote_mask(R, 4)
        # From the second comma to the newline: the campaign, a comma, the ts, the newline.
        digits = R.choice(edge) - 2 - len(f[2]) - 2 * (mask >> 2 & 1) - 2 * (mask >> 3 & 1)
        if R.random() < 0.8 and digits >= len(str(ms)):
            f[3] = str(ms).zfill(digits)
        for line in lines:
            line.f = list(f)
            line.changed = True
            line.quotes = mask
            touched.add(id(line))
        click_placed += 1
    click_shorts = ["", "a", ",,,", '"",,,', "x,y", ",", "abcdefghij", ",,,,"]
    out = []
    for line in clicks:
        out.append(line)
        if id(line) in touched and not line.held and line.members is None and R.random() < 0.15:
            for _ in range(R.choice([1, 1, 2, 3])):
                out.append(Line(R.choice(click_shorts), None, line.file))
                after += 1
    D.clicks = out
    D.notes.append(f"windows {len(mapping)} users, {long_ids} event ids, {placed} event lines, {click_placed} clicks, {after} short lines after")


def grouping(D):
    """The lines of a file in another order (the order of lines means nothing,
    so the answer is the one before): all lines whose user is 33 bytes long
    together, all whose event or click ID is, both; such a line in every
    second place, or every eighth, or every ninth; the lines by length, the
    lines with a quoted ID first. One ID in eight is long, so a decoder that
    takes eight lines at a time hardly ever sees a batch of nothing but long
    IDs, or the long ID always in the same lane, unless a test puts them
    there. (After the long-ID mutations the 33-byte near misses are grouped
    with the long IDs: the batch then is all lines that only look long.)"""
    R = D.R

    def regroup(lines, id_at, user_at):
        """`lines` (Line objects of one file) in a new order."""
        def is33(line, at):
            f = line.f
            return f is not None and len(f[at]) == 33

        style = R.choice(["users", "ids", "both", "both", "period2", "period8", "period9", "length", "length_down", "quoted", "blocks",
                          "period63", "period64", "period64", "period65", "allbut64", "allbut64", "runs64", "userslast", "userslast"])
        if style == "users":
            return sorted(lines, key=lambda l: not is33(l, user_at)), style
        if style == "userslast":
            # The file's last lines, and its very last one, have a 33-byte user.
            return sorted(lines, key=lambda l: is33(l, user_at)), style
        if style == "ids":
            return sorted(lines, key=lambda l: not is33(l, id_at)), style
        if style == "both":
            return sorted(lines, key=lambda l: (not (is33(l, id_at) and is33(l, user_at)), not is33(l, user_at), not is33(l, id_at))), style
        if style.startswith("period") or style in ("allbut64", "runs64"):
            # One such line in every 2, 8, 9, 63, 64, 65 (a batch of 64 lines
            # then has one, at a place that stays or creeps); all but one in
            # every 64; 64 with and 64 without in turn. For the long steps it
            # is the user that counts, three times in four.
            every = 64 if style in ("allbut64", "runs64") else int(style[6:])
            at = user_at if every > 9 and R.random() < 0.75 else R.choice([id_at, user_at])
            special = [l for l in lines if is33(l, at)]
            plain = [l for l in lines if not is33(l, at)]
            out, phase = [], R.randrange(every)
            while special or plain:
                if style == "runs64":
                    want_special = len(out) // 64 % 2 == 0
                else:
                    want_special = (len(out) % every == phase) != (style == "allbut64")
                source = special if (want_special and special) or not plain else plain
                out.append(source.pop())
            return out, style
        if style in ("length", "length_down"):
            def size(line):
                return len(line.raw) if line.raw is not None and not line.changed else sum(len(v) for v in (line.f or [])) + 6
            return sorted(lines, key=size, reverse=style == "length_down"), style
        if style == "quoted":
            return sorted(lines, key=lambda l: not (l.raw or "").startswith('"')), style
        # Runs of 1 to 40 long ones, then as many others, and so on.
        special = [l for l in lines if is33(l, id_at) or is33(l, user_at)]
        plain = [l for l in lines if not (is33(l, id_at) or is33(l, user_at))]
        out = []
        while special or plain:
            for source in (special, plain):
                for _ in range(R.choice([1, 2, 7, 8, 9, 16, 17, 40])):
                    if source:
                        out.append(source.pop())
        return out, style

    notes = []
    for name, all_lines in (("events", D.events), ("clicks", D.load_clicks())):
        by_file = {}
        for at, line in enumerate(all_lines):
            # Whole lines only: a block and the lines it holds stay where they are.
            if line.members is None and not line.held and not line.gone:
                by_file.setdefault(line.file, []).append(at)
        files = sorted(by_file, key=lambda n: len(by_file[n]), reverse=True)
        chosen = files[:3] + R.sample(files[3:], min(len(files[3:]), max(2, len(files) // 2)))
        done = []
        for number in chosen:
            places = by_file[number]
            if len(places) < 4:
                continue
            new, style = regroup([all_lines[at] for at in places], 0, 1)
            for at, line in zip(places, new):
                all_lines[at] = line
                # (A new line would be put at a random place when written.)
                line.new = False
            done.append(style)
        notes.append(f"{name} {len(done)} files ({', '.join(sorted(set(done)))})")
    D.notes.append("grouping " + "; ".join(notes))


def longlinks(D):
    """Link lines for a reader with a short cut for long ones (two 64-byte
    windows: the first ID and the comma, the second ID and the newline; both
    IDs of the long form by their value). Every line links a user who has rows
    to a new ID, or two users who have rows, so a link read wrongly shows in
    the answer: the two people are one or are not, and the smallest of their
    IDs, byte by byte, is the name in the output.

    The new IDs are of the long form, of the forms near it (32 and 34 bytes,
    upper case, a letter that is no hex digit, a NUL), and of every length
    that puts the comma at byte 29 to 35 or 60 to 66 and the newline as many
    bytes after the comma, with and without quotes (a long ID in quotes is 35
    bytes); lines of exactly 30 to 34 bytes made of two short IDs. Both sides
    long, one side, either way round, the pair once more the other way round,
    an ID linked to itself; chains a-b, b-c, c-d of long and short IDs between
    two people. And lines that link nobody: a quote inside an ID, three
    fields, one field, an empty side, with users who have rows on both sides
    (read as links, they would make one person of two)."""
    R = D.R
    if not D.link_files:
        D.notes.append("longlinks: no link files")
        return
    L = Longs(R, [u[1:] for u in D.users if is_long(u, "u")])
    users = list(D.users)
    taken = D.all_users()
    edges = [29, 30, 31, 31, 32, 32, 33, 33, 34, 35, 60, 61, 62, 62, 62, 63, 63, 63, 64, 64, 64, 65, 65, 65, 66, 67]

    def put(a, b, qa=None, qb=None):
        qa = R.random() < 0.3 if qa is None else qa
        qb = R.random() < 0.3 if qb is None else qb
        D.links.append([R.randrange(len(D.link_files)), ('"' + a + '"' if qa else a) + "," + ('"' + b + '"' if qb else b)])

    def fresh(size, small=None):
        """A new ID of `size` bytes; `small`: one that sorts before every
        generated ID (it becomes the person's name), or after."""
        small = R.random() < 0.5 if small is None else small
        for _ in range(20):
            head = R.choice(["U", "A", "u!", "u0", "0", "u "]) if small else R.choice(["u", "uz", "u~", "v", "z"])
            body = "".join(R.choice(DIGITS) for _ in range(max(0, size - len(head))))
            new = (head + body)[:size]
            if new and new not in taken:
                taken.add(new)
                return new
        return None

    made = 0
    # A user with rows and a new ID of a length that puts the comma, or the
    # newline, at an edge; quoted or not on either side.
    for _ in range(max(500, len(users) // 2)):
        user = R.choice(users)
        r = R.random()
        if r < 0.25:
            alias = "u" + L.hex()
        elif r < 0.40:
            alias = L.near("u", text=True)
        elif r < 0.50 and is_long(user, "u"):
            alias = R.choice(relatives(L, user))
        else:
            qa = R.random() < 0.3
            alias = fresh(R.choice(edges) - 2 * qa)
            if alias is None:
                continue
            if R.random() < 0.5:
                put(alias, user, qa=qa)
            else:
                put(user, alias, qb=qa)
            made += 1
            continue
        if not sayable(alias) or alias == user:
            continue
        taken.add(alias)
        if R.random() < 0.5:
            put(alias, user)
        else:
            put(user, alias)
        r = R.random()
        if r < 0.15:
            put(user, alias)                       # once more, perhaps the other way round
        elif r < 0.30:
            put(alias, alias)                      # linked to itself
        elif r < 0.40:
            put(user, user)
        if R.random() < 0.15:
            D.new_event("e" + L.hex(), user=alias)   # the new ID has a row of its own
        made += 1
    # Lines of exactly 30 to 34 bytes made of two IDs that are not long (the
    # short cut starts at a length), one of them a user with rows.
    short = 0
    for _ in range(max(150, len(users) // 8)):
        user = R.choice(users)
        qa, qb = R.random() < 0.4, R.random() < 0.4
        size = R.choice([30, 31, 31, 32, 32, 33, 33, 34]) - 1 - len(user) - 2 * qa - 2 * qb
        if size < 1:
            continue
        alias = fresh(size)
        if alias is None:
            continue
        if R.random() < 0.5:
            put(alias, user, qa=qa, qb=qb)
        else:
            put(user, alias, qa=qb, qb=qa)
        short += 1
    # Two people made one: both sides are users with rows.
    merged = 0
    longs = [u for u in users if is_long(u, "u")]
    for _ in range(max(10, len(users) // 100)):
        a = R.choice(longs) if longs and R.random() < 0.7 else R.choice(users)
        b = R.choice(longs) if longs and R.random() < 0.5 else R.choice(users)
        if a != b:
            put(a, b)
            if R.random() < 0.3:
                put(b, a)
            merged += 1
    # Chains of new IDs, long and short, between two users with rows.
    chains = 0
    for _ in range(max(6, len(users) // 300)):
        chain = [R.choice(users)]
        for _ in range(R.randint(2, 14)):
            r = R.random()
            new = "u" + L.hex() if r < 0.45 else fresh(R.choice([2, 3, 9, 10, 20, 32, 34, 40]), small=R.random() < 0.3)
            if new and sayable(new):
                taken.add(new)
                chain.append(new)
        chain.append(R.choice(users))
        pairs = list(zip(chain, chain[1:]))
        R.shuffle(pairs)
        for a, b in pairs:
            if R.random() < 0.5:
                a, b = b, a
            put(a, b)
        chains += 1
    # Lines that link nobody, between two users with rows.
    bad = 0
    for _ in range(max(60, len(users) // 15)):
        a, b = R.choice(longs) if longs and R.random() < 0.6 else R.choice(users), R.choice(longs) if longs and R.random() < 0.6 else R.choice(users)
        if a == b:
            continue
        text = R.choice([
            f"{a},{b},", f"{a},{b},x", f",{a},{b}", f"{a},,{b}",        # three fields
            f"{broken(R, a)},{b}", f"{a},{broken(R, b)}",              # a quote where none may be
            f"{a},", f",{b}", f'{a},""', f'"",{b}', f'"{a}",', ",", '"",""',   # an empty side
            f"{a}", f"{a} {b}", f"{a};{b}",                            # one field
            f'"{a},{b}"', f'"{a}","{b}",""', f"{a},{b}\r,{a}",
        ])
        D.links.append([R.randrange(len(D.link_files)), text])
        bad += 1
    D.notes.append(f"longlinks {made} to new IDs, {short} short lines, {merged} merges, {chains} chains, {bad} that link nobody")


MUTATIONS = {f.__name__: f for f in [irregular, outrange, copies, disorder, gaps, nested, links, wide, drop, irrfile, badhead,
                                     purchases, shapes, clickshapes, linkshapes, userlengths, lateness, values, stretches,
                                     splitter, clicksplit, interleave, endings, identityids, emptyevents, noeventfiles,
                                     idbytes, longusers, longevents, longclicks, windows, grouping, longlinks]}
MIXABLE = ["irregular", "outrange", "copies", "disorder", "gaps", "nested", "links", "wide", "drop", "irrfile", "badhead",
           "idbytes", "idbytes", "longusers", "longusers", "longevents", "longevents", "longclicks", "longclicks",
           "windows", "windows", "grouping", "grouping", "longlinks", "longlinks",
           "purchases", "shapes", "shapes", "clickshapes", "linkshapes", "userlengths", "lateness", "values", "stretches",
           "splitter", "splitter", "clicksplit", "clicksplit", "interleave", "endings"]


# ------------------------------------------------------------------ writing

def encode(D, fields, quotes=None):
    if quotes is not None:
        return ",".join('"' + v + '"' if quotes >> i & 1 else v for i, v in enumerate(fields))
    return ",".join('"' + v + '"' if D.R.random() < 0.2 else v for v in fields)


def body(D, header, lines):
    """A file's bytes; after `endings`, some files lack the last newline and
    some end with an empty line."""
    text = header + "\n" + "".join(l + "\n" for l in lines)
    if D.endings and lines:
        r = D.R.random()
        if r < 0.3:
            text = text[:-1]
        elif r < 0.5:
            text += "\n"
    return text.encode("latin-1")


def write(D, out_dir):
    R = D.R
    os.makedirs(os.path.join(out_dir, "events"), exist_ok=True)
    per_file = [[] for _ in D.event_files]
    added = [[] for _ in D.event_files]
    for line in D.events:
        if not line.gone and not line.held:
            (added if line.new else per_file)[line.file].append(line)   # a line of the input keeps its place
    for number, extra in enumerate(added):
        for line in extra:
            per_file[number].insert(R.randrange(len(per_file[number]) + 1), line)
    # What the rules promise: copies with equal ingest_ts are identical. A line
    # that would break it (two mutations met on one event) is left out.
    seen = {}

    def emit(line, out):
        if line.f is None:
            out.append(line.raw)
            return
        text = encode(D, line.f, line.quotes) if line.changed or line.raw is None else line.raw
        if line.f[0]:
            key = (line.f[0], line.f[3])
            if seen.setdefault(key, line.f) != line.f:
                return
        out.append(text)

    texts = []
    for number, lines in enumerate(per_file):
        out = []
        for line in lines:
            if line.members is None:
                emit(line, out)
                continue
            for member in line.members:
                if isinstance(member, str):
                    out.append(member)
                elif not member.gone:
                    emit(member, out)
        texts.append(out)
    if not D.drop_event_files:
        for (name, header, _), lines in zip(D.event_files, texts):
            with open(os.path.join(out_dir, "events", name), "wb") as f:
                f.write(body(D, header or EVENT_HEADER, lines))
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
            f.write(body(D, header or "user_id_a,user_id_b", lines))
    if D.clicks is None and not D.endings:
        shutil.copytree(os.path.join(D.in_dir, "clicks"), os.path.join(out_dir, "clicks"))
    else:
        if D.clicks is None:
            D.clicks = [Line(raw, None, number) for number, (_, _, lines) in enumerate(D.click_files) for raw in lines]
        os.makedirs(os.path.join(out_dir, "clicks"), exist_ok=True)
        click_lines = [[] for _ in D.click_files]
        for line in D.clicks:
            if line.held:
                continue
            for member in [line] if line.members is None else line.members:
                if isinstance(member, str):
                    click_lines[line.file].append(member)
                else:
                    click_lines[line.file].append(encode(D, member.f, member.quotes) if member.f is not None and member.changed else member.raw)
        for (name, header, _), lines in zip(D.click_files, click_lines):
            with open(os.path.join(out_dir, "clicks", name), "wb") as f:
                f.write(body(D, header or "click_id,user_id,campaign_id,ts", lines))
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
