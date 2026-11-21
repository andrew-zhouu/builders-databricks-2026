#!/usr/bin/env bash
# Run the organisers' own test scripts (polars-based, as the scorer's checker
# presumably is) against the binary: all fixtures, then the dev answer. Also
# reads the output with DuckDB as a third independent Parquet reader.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
PY="$PROJ/.venv/bin/python"
ST="$PROJ/official/starter-duckdb/x/starter-duckdb"
"$PY" "$ST/scripts/run_fixtures.py" "$ST/fixtures" -- "$BIN" | tail -3
out="$PROJ/out/official-dev"; rm -rf "$out"; mkdir -p "$out"
"$BIN" "$PROJ/data/dev" "$out" || exit 1
"$PY" "$ST/scripts/compare.py" "$out" "$PROJ/official/dev-answer/x/dev-answer"
"$PY" -I - "$out" <<'EOF'
import sys, duckdb, polars as pl, pyarrow.parquet as pq
out = sys.argv[1]
for t in ("user_daily", "campaign_daily"):
    n = duckdb.sql(f"select count(*) from read_parquet('{out}/{t}/*.parquet')").fetchall()[0][0]
    df = pl.read_parquet(f"{out}/{t}/*.parquet")
    md = pq.ParquetFile(sorted(__import__('glob').glob(f"{out}/{t}/*.parquet"))[0]).schema
    print(t, "duckdb rows", n, "polars rows", df.height, "polars schema", dict(df.schema))
print(duckdb.sql(f"select sum(sessions) s, sum(revenue_cents) r from read_parquet('{out}/user_daily/*.parquet')").fetchall(),
      duckdb.sql(f"select sum(sessions) s, sum(revenue_cents) r, sum(assisted_sessions) a from read_parquet('{out}/campaign_daily/*.parquet')").fetchall())
EOF
