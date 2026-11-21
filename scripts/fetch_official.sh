#!/usr/bin/env bash
# Download and unpack the official competition materials into official/ (not
# committed): rules, track page, starters with the 36 fixtures, the published
# dev answer and the dataset generator. Each archive gets its own directory.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
B=https://www.joinbuilders.org/buildathon/tracks/shopping-sessions
UA="Mozilla/5.0 (X11; Linux x86_64)"
cd "$PROJ"
mkdir -p official/rules official/page
curl -sSL -A "$UA" -o official/rules/rules.md -w "rules %{http_code} %{size_download}\n" "$B/files/rules.md"
curl -sSL -A "$UA" -o official/page/page.html -w "page %{http_code} %{size_download}\n" "$B"
for f in starter-duckdb starter-polars starter-pyspark dev-answer generator; do
  mkdir -p "official/$f/x"
  curl -sSL -A "$UA" -o "official/$f/$f.zip" -w "$f %{http_code} %{size_download}\n" "$B/files/$f.zip"
  python3 -I -c "import sys,zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" "official/$f/$f.zip" "official/$f/x"
done
