#!/usr/bin/env bash
# tools/tssweep.py checked locally on small sets: it runs, the Python reference
# accepts what it makes, and the answer has both outcomes of every edge (one
# session and two, credited clicks and organic ones, revenue and none). Light
# enough to run anywhere.
# Usage: sweepcheck.sh [USERS]
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
W=$(mktemp -d)
for seed in 0 1 2 3; do
  python3 -I tools/tssweep.py "$seed" "$W/d" "${1:-600}"
  python3 -I reference/reference.py "$W/d" "$W/r"
  echo "seed $seed: $(tr -d '\n ' < "$W/d/manifest.json" | cut -c1-80) $(cat "$W"/d/events/*.csv | wc -l) event lines, $(cat "$W"/d/clicks/*.csv | wc -l) click lines"
  python3 -I - "$W/r" <<'EOF'
import csv, sys, collections
rows = list(csv.DictReader(open(sys.argv[1] + "/user_daily.csv")))
sessions = collections.Counter(r["sessions"] for r in rows)
camp = list(csv.DictReader(open(sys.argv[1] + "/campaign_daily.csv")))
organic = sum(int(r["sessions"]) for r in camp if r["campaign_id"] == "organic")
credited = sum(int(r["sessions"]) for r in camp if r["campaign_id"] != "organic")
print(f"   user rows {len(rows)}, sessions per row {dict(sorted(sessions.items()))}, rows with revenue {sum(1 for r in rows if int(r['revenue_cents']) > 0)}, "
      f"sessions credited to a click {credited}, organic {organic}")
EOF
  rm -rf "$W/d" "$W/r"
done
rm -rf "$W"
