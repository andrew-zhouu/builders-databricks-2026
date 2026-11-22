#!/usr/bin/env python3
"""
Exact, order-insensitive, byte-level comparison of two outputs.

Each side is a folder holding either {user_daily,campaign_daily}.csv (the
published expected layout: raw bytes, empty top_category = null) or
{user_daily,campaign_daily}/*.parquet (a pipeline's output).

Usage: cmp.py A B        exit 0 = identical
"""
import datetime
import os
import sys

import pyarrow as pa
import pyarrow.parquet as pq

TABLES = {
    "user_daily": ["user_id", "day", "sessions", "events", "revenue_cents", "top_category"],
    "campaign_daily": ["campaign_id", "channel", "day", "sessions", "purchasing_sessions",
                       "revenue_cents", "assisted_sessions"],
}
STRINGS = {"user_id", "top_category", "campaign_id", "channel"}
NULLABLE = {"top_category"}


def load_csv(path, columns):
    rows = []
    with open(path, "rb") as f:
        lines = f.read().split(b"\n")
    if lines and lines[-1] == b"":
        lines.pop()
    assert lines[0] == ",".join(columns).encode(), f"{path}: header {lines[0]!r}"
    for line in lines[1:]:
        parts = line.split(b",")
        assert len(parts) == len(columns), f"{path}: {line!r}"
        row = []
        for name, value in zip(columns, parts):
            if name in STRINGS:
                row.append(None if (name in NULLABLE and value == b"") else value)
            elif name == "day":
                row.append(value.decode())
            else:
                row.append(int(value))
        rows.append(tuple(row))
    return rows


def load_parquet(folder, columns):
    rows = []
    files = sorted(n for n in os.listdir(folder)
                   if n.endswith(".parquet") and not n.startswith((".", "_"))
                   and os.path.isfile(os.path.join(folder, n)) and not os.path.islink(os.path.join(folder, n)))
    assert files, f"no parquet in {folder}"
    for name in files:
        table = pq.read_table(os.path.join(folder, name))
        assert set(table.column_names) == set(columns), f"{name}: columns {table.column_names}"
        cols = []
        for column in columns:
            typ = table.schema.field(column).type
            data = table.column(column)
            if column in STRINGS and str(typ) in ("string", "large_string"):
                # As bytes: a string that is not UTF-8 (the input's, written out
                # faithfully, or a pipeline's mistake) must be compared like
                # any other, not stop the comparison.
                data = data.cast(pa.large_binary() if str(typ) == "large_string" else pa.binary())
            values = data.to_pylist()
            if column in STRINGS:
                values = [None if v is None else (v.encode() if isinstance(v, str) else bytes(v)) for v in values]
            elif column == "day":
                assert str(typ) == "date32[day]", f"{name}: day is {typ}"
                values = [v.isoformat() if isinstance(v, datetime.date) else v for v in values]
            else:
                assert str(typ).startswith(("int", "uint")), f"{name}: {column} is {typ}"
            if column not in NULLABLE:
                assert all(v is not None for v in values), f"{name}: null in {column}"
            cols.append(values)
        rows.extend(zip(*cols))
    return rows


def load(folder, table):
    csv = os.path.join(folder, table + ".csv")
    if os.path.isfile(csv):
        return load_csv(csv, TABLES[table])
    return load_parquet(os.path.join(folder, table), TABLES[table])


def key(row):
    return tuple((0, b"") if v is None else (1, v) if isinstance(v, bytes) else (2, str(v).encode()) for v in row)


def main():
    a_dir, b_dir = sys.argv[1], sys.argv[2]
    same = True
    for table in TABLES:
        a, b = load(a_dir, table), load(b_dir, table)
        sa, sb = sorted(a, key=key), sorted(b, key=key)
        if sa == sb:
            print(f"{table}: {len(a)} rows identical")
            continue
        same = False
        set_a, set_b = set(a), set(b)
        only_a = sorted(set_a - set_b, key=key)
        only_b = sorted(set_b - set_a, key=key)
        print(f"{table}: rows {len(a)} vs {len(b)}; only in A {len(only_a)}; only in B {len(only_b)}; "
              f"dup rows {len(a) - len(set_a)} vs {len(b) - len(set_b)}")
        for row in only_a[:8]:
            print("   A:", row)
        for row in only_b[:8]:
            print("   B:", row)
    print("MATCH" if same else "DIFFER")
    return 0 if same else 1


if __name__ == "__main__":
    sys.exit(main())
