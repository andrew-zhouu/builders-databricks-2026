#!/usr/bin/env bash
# Runs ON the remote machine: can every common Parquet reader read our output,
# and do they all see the same rows as the reference? Runs the current build on
# a data set, then reads user_daily and campaign_daily with pyarrow, polars,
# duckdb and fastparquet (whichever are installed), and with the organisers'
# compare script against the dev answer when the data set is `dev`.
# Usage: readers.sh [DATASET]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-dev}"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
out=/tmp/ss-readers; rm -rf "$out"; mkdir -p "$out"
NCPU=8 "$BIN" "data/$name" "$out" 2>/dev/null || { echo "run failed"; exit 1; }
du -sh "$out/user_daily" "$out/campaign_daily" | tr '\n' ' '; echo
.venv/bin/python -I -m pip install -q duckdb fastparquet > /dev/null 2>&1 || true
.venv/bin/python -I - "$out" <<'EOF'
import sys, glob, hashlib
out = sys.argv[1]
def digest(rows):
    h = hashlib.sha256()
    for r in sorted(rows, key=repr): h.update(repr(r).encode())
    return h.hexdigest()[:16], len(rows)
results = {}
for table in ("user_daily", "campaign_daily"):
    files = sorted(glob.glob(f"{out}/{table}/*.parquet"))
    try:
        import pyarrow.parquet as pq, pyarrow as pa
        t = pa.concat_tables([pq.read_table(f) for f in files])
        cols = t.to_pydict(); names = t.column_names
        rows = list(zip(*[cols[n] for n in names]))
        results[(table, "pyarrow " + pa.__version__)] = digest([tuple(str(x) for x in r) for r in rows])
        md = pq.ParquetFile(files[0]).metadata
        if md.num_row_groups:
            print(table, "encodings:", {md.schema.column(i).name: md.row_group(0).column(i).encodings for i in range(md.num_columns)})
    except Exception as e: print(table, "pyarrow FAILED:", repr(e)[:200])
    try:
        import polars as pl
        df = pl.concat([pl.read_parquet(f) for f in files])
        results[(table, "polars " + pl.__version__)] = digest([tuple(str(x) for x in r) for r in df.rows()])
    except Exception as e: print(table, "polars FAILED:", repr(e)[:200])
    try:
        import duckdb
        rows = duckdb.sql(f"select * from read_parquet('{out}/{table}/*.parquet')").fetchall()
        results[(table, "duckdb " + duckdb.__version__)] = digest([tuple(str(x) for x in r) for r in rows])
    except Exception as e: print(table, "duckdb FAILED:", repr(e)[:200])
    try:
        import fastparquet, pandas as pd
        df = pd.concat([fastparquet.ParquetFile(f).to_pandas() for f in files])
        results[(table, "fastparquet " + fastparquet.__version__)] = (None, len(df))
    except Exception as e: print(table, "fastparquet FAILED:", repr(e)[:200])
for k, v in results.items(): print(f"{k[0]:15s} {k[1]:22s} rows {v[1]:9d} digest {v[0]}")
EOF
if [ "$name" = dev ]; then
  ST="$PROJ/official/starter-polars/x/starter-polars"
  (cd "$ST/scripts" && "$PROJ/.venv/bin/python" compare.py "$PROJ/official/dev-answer/x/dev-answer" "$out" 2>&1 | tail -3)
fi
