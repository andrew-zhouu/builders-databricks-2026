#!/usr/bin/env bash
# Sanity-check what one fuzz dataset contains and how the binary routes its lines.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
d=/tmp/ss-fuzzstat; rm -rf "$d"; mkdir -p "$d"
"$PROJ/.venv/bin/python" -I "$PROJ/tools/fuzz.py" "${1:-5}" "$d/data" "${2:-3000}"
cat "$d/data/manifest.json"; echo
wc -l "$d"/data/events/*.csv "$d"/data/clicks/*.csv "$d"/data/links/*.csv | tail -1
"$PROJ/.venv/bin/python" -I "$PROJ/reference/reference.py" "$d/data" "$d/expected"
wc -l "$d/expected/user_daily.csv" "$d/expected/campaign_daily.csv"
head -5 "$d/expected/user_daily.csv" | cat -A | cut -c1-100
awk -F, 'NR>1{s+=$3; e+=$4; r+=$5; if($6!="")c++} END{print "sessions",s,"events",e,"revenue",r,"rows with category",c}' "$d/expected/user_daily.csv"
awk -F, 'NR>1{s+=$4; a+=$7; if($1=="organic")o+=$4} END{print "credited sessions",s,"organic",o,"assists",a}' "$d/expected/campaign_daily.csv"
STATS=1 SS_TIMING=1 NCPU=2 "$BIN" "$d/data" "$d/out" 2>&1 | tail -3
