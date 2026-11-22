#!/usr/bin/env bash
# Download the official materials again into a fresh directory under out/
# (not committed) and show what differs from the copy in official/.
# Usage: refetch.sh [NAME]      (default: a time stamp)
set -u
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
B=https://www.joinbuilders.org/buildathon/tracks/shopping-sessions
UA="Mozilla/5.0 (X11; Linux x86_64)"
cd "$PROJ"
N="out/official-${1:-$(date -u +%m%d-%H%M)}"
rm -rf "$N"; mkdir -p "$N/rules" "$N/page"
curl -sSL -A "$UA" -o "$N/rules/rules.md" -w "rules %{http_code} %{size_download}\n" "$B/files/rules.md"
curl -sSL -A "$UA" -o "$N/page/page.html" -w "page %{http_code} %{size_download}\n" "$B"
for f in starter-duckdb starter-polars starter-pyspark dev-answer generator; do
  mkdir -p "$N/$f/x"
  curl -sSL -A "$UA" -o "$N/$f/$f.zip" -w "$f %{http_code} %{size_download}\n" "$B/files/$f.zip"
  python3 -I -c "import sys,zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "$N/$f/$f.zip" "$N/$f/x" 2>&1 | tail -1
done
echo "== sizes then and now"
for f in rules/rules.md page/page.html starter-duckdb/starter-duckdb.zip starter-polars/starter-polars.zip starter-pyspark/starter-pyspark.zip dev-answer/dev-answer.zip generator/generator.zip; do
  printf '%-44s %10s %10s  %s\n' "$f" "$(stat -c %s "official/$f" 2>/dev/null)" "$(stat -c %s "$N/$f" 2>/dev/null)" "$(cmp -s "official/$f" "$N/$f" && echo same || echo DIFFERENT)"
done
echo "== files that differ (unpacked)"
diff -rq official "$N" 2>&1 | grep -v '\.zip differ' | head -60
echo "new copy in $N"
