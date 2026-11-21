#!/usr/bin/env bash
# Download the official competition materials, each into its own directory.
set -u
ROOT=/home/andre/builders/databricks
B=https://www.joinbuilders.org/buildathon/tracks/shopping-sessions
UA="Mozilla/5.0 (X11; Linux x86_64)"
cd "$ROOT"
mkdir -p official/rules official/page
curl -sSL -A "$UA" -o official/rules/rules.md -w "rules %{http_code} %{size_download}\n" "$B/files/rules.md"
curl -sSL -A "$UA" -o official/page/page.html -w "page %{http_code} %{size_download}\n" "$B"
for f in starter-duckdb starter-polars starter-pyspark dev-answer generator; do
  mkdir -p "official/$f"
  curl -sSL -A "$UA" -o "official/$f/$f.zip" -w "$f %{http_code} %{size_download}\n" "$B/files/$f.zip"
done
ls -la official/*
