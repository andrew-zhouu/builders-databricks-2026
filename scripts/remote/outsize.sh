#!/usr/bin/env bash
# Runs ON the remote machine: how large is the output, table by table and
# column by column? Runs a binary on a data set and reads the Parquet metadata.
# Usage: outsize.sh [DATASET] [BINARY]        env passes through (SS_OUT_CHUNK=...)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"
BIN="${2:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
out=/tmp/ss-outsize; rm -rf "$out"; mkdir -p "$out"
NCPU=8 "$BIN" "data/$name" "$out" 2>/dev/null || { echo "run failed"; exit 1; }
.venv/bin/python -I - "$out" <<'EOF'
import sys, glob, os, collections
import pyarrow.parquet as pq
out = sys.argv[1]
for table in ("user_daily", "campaign_daily"):
    files = sorted(glob.glob(f"{out}/{table}/*.parquet"))
    total = sum(os.path.getsize(f) for f in files)
    cols = collections.Counter(); dicts = collections.Counter(); rows = 0
    for f in files:
        md = pq.ParquetFile(f).metadata
        rows += md.num_rows
        for g in range(md.num_row_groups):
            for c in range(md.num_columns):
                col = md.row_group(g).column(c)
                cols[col.path_in_schema] += col.total_compressed_size
                if col.dictionary_page_offset is not None:
                    dicts[col.path_in_schema] += col.data_page_offset - col.dictionary_page_offset
    print(f"{table}: {total/1e6:.2f} MB in {len(files)} files, {rows} rows")
    for k, v in cols.most_common():
        print(f"   {k:20s} {v/1e6:7.2f} MB  (dictionary {dicts[k]/1e6:.2f})")
EOF
rm -rf "$out"
