#!/usr/bin/env python3
"""
Shrink a data set on which a binary under some settings differs from its own
plain implementation (--slow): remove event lines, then click lines, then
link lines, for as long as the two still differ (delta debugging), and write
what is left.

Usage: shrink.py DATA_DIR OUT_DIR BINARY [VAR=VALUE ...]
The settings go to the fast run only. Give SS_INDEX_KEY (and SS_INDEX_BOUND)
when the difference needs the index path: with few lines left nothing can
infer a key from the data. Prints the surviving lines at the end.
"""
import os
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cmp as compare  # noqa: E402  (tools/cmp.py)

HEADERS = {"events": "event_id,user_id,ts,ingest_ts,event_type,product_id,price_cents", "clicks": "click_id,user_id,campaign_id,ts",
           "links": "user_id_a,user_id_b"}


def read(folder):
    files = []
    for name in sorted(os.listdir(folder)) if os.path.isdir(folder) else []:
        if name.endswith(".csv"):
            with open(os.path.join(folder, name), "rb") as f:
                lines = f.read().decode("latin-1").split("\n")
            if lines and lines[-1] == "":
                lines.pop()
            files.append(lines[1:])
    return files


class Case:
    def __init__(self, data, binary, settings):
        self.data, self.binary = data, binary
        self.env = dict(os.environ, **dict(s.split("=", 1) for s in settings))
        self.parts = {kind: read(os.path.join(data, kind)) for kind in HEADERS}
        self.work = tempfile.mkdtemp(prefix="ss-shrink-")
        self.tests = 0

    def write(self, out):
        if os.path.exists(out):
            shutil.rmtree(out)
        os.makedirs(out)
        for name in ("products.csv", "campaigns.csv", "manifest.json"):
            shutil.copy(os.path.join(self.data, name), os.path.join(out, name))
        for kind, files in self.parts.items():
            os.makedirs(os.path.join(out, kind))
            for number, lines in enumerate(files):
                with open(os.path.join(out, kind, f"part-{number:05d}.csv"), "wb") as f:
                    f.write((HEADERS[kind] + "\n" + "".join(l + "\n" for l in lines)).encode("latin-1"))

    def differs(self):
        """Do the fast run and the plain implementation disagree (or the fast run fail)?"""
        self.tests += 1
        d = os.path.join(self.work, "d")
        self.write(d)
        slow, fast = os.path.join(self.work, "slow"), os.path.join(self.work, "fast")
        for path in (slow, fast):
            shutil.rmtree(path, ignore_errors=True)
        if subprocess.run([self.binary, "--slow", d, slow], capture_output=True).returncode != 0:
            return False                      # not a data set the yardstick accepts
        try:
            if subprocess.run([self.binary, d, fast], capture_output=True, env=self.env, timeout=120).returncode != 0:
                return True
        except subprocess.TimeoutExpired:
            return True
        try:
            for table in compare.TABLES:
                a, b = compare.load(slow, table), compare.load(fast, table)
                if sorted(a, key=compare.key) != sorted(b, key=compare.key):
                    return True
        except Exception:
            return True
        return False

    def minimise(self, kind):
        """ddmin over the lines of one folder, file by file kept apart."""
        files = self.parts[kind]
        flat = [(number, line) for number, lines in enumerate(files) for line in lines]

        def put(items):
            self.parts[kind] = [[l for n, l in items if n == number] for number in range(len(files))]

        chunks = 2
        while len(flat) >= 1:
            size = max(1, len(flat) // chunks)
            reduced = False
            for start in range(0, len(flat), size):
                rest = flat[:start] + flat[start + size:]
                put(rest)
                if self.differs():
                    flat, reduced = rest, True
                    chunks = max(2, chunks - 1)
                    break
            if not reduced:
                if size == 1:
                    break
                chunks = min(len(flat), chunks * 2)
        put(flat)
        return len(flat)


def main():
    data, out, binary, settings = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4:]
    case = Case(data, binary, settings)
    if not case.differs():
        print("the two do not differ on this data set under these settings")
        sys.exit(1)
    left = {kind: case.minimise(kind) for kind in ("events", "clicks", "links")}
    left["events"] = case.minimise("events")       # once more: fewer clicks and links may free more
    case.write(out)
    print(f"shrunk to {left} in {case.tests} runs: {out}")
    for kind, files in case.parts.items():
        for number, lines in enumerate(files):
            for line in lines:
                print(f"  {kind}/part-{number:05d}: {line}")
    with open(os.path.join(data, "manifest.json")) as f:
        print("  manifest:", f.read().strip())
    shutil.rmtree(case.work, ignore_errors=True)


if __name__ == "__main__":
    main()
