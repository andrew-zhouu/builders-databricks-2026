#!/usr/bin/env bash
# Unpack each official archive into its own directory and list contents.
set -u
ROOT=/home/andre/builders/databricks/official
for f in starter-duckdb starter-polars starter-pyspark dev-answer generator; do
  echo "=== $f ==="
  mkdir -p "$ROOT/$f/x"
  unzip -o -q "$ROOT/$f/$f.zip" -d "$ROOT/$f/x" || python3 -I -c "import sys,zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$ROOT/$f/$f.zip" "$ROOT/$f/x"
  find "$ROOT/$f/x" -type f -printf '%10s  %P\n' | sort -k2 | head -100
done
