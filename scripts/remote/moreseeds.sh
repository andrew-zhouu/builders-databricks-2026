#!/usr/bin/env bash
# Runs ON the remote machine: more ground for the final, which is new data at
# three times the practice size. Finds a seed whose window holds 29 February
# 2028, then checks a binary on new sets against the plain implementation (and,
# on the small set, both against the Python reference).
#   leap seed:  200K (Python reference), 5M and 20M
#   seed 31:    20M        seed 77: 60M
# Usage: moreseeds.sh [BINARY]      (default: the current build)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
export BIN="${1:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
rc=0
leap=$(cat out/leap-seed 2>/dev/null || true)
if [ -z "$leap" ]; then
  for seed in $(seq 100 4000); do
    d=/tmp/ss-leap; rm -rf "$d"
    generate --seed "$seed" --events 2000 --clicks 500 --output "$d" --threads 2 > /dev/null 2>&1
    ws=$(tr -d ' \n' < "$d/manifest.json" | sed 's/.*"window_start":"\([0-9-]*\)T.*/\1/')
    # A 14-day window holds 2028-02-29 iff it starts on 16 to 29 February 2028.
    case "$ws" in 2028-02-1[6-9]|2028-02-2[0-9]) leap=$seed; break ;; esac
  done
  rm -rf /tmp/ss-leap
  [ -n "$leap" ] || { echo "no leap-day seed below 4000"; exit 1; }
  echo "$leap" > out/leap-seed
fi
echo "leap-day seed: $leap"
bash scripts/seeds.sh 200000 50000 "$leap" || rc=1

large() { # name seed events clicks
  local name="$1" exp="expected/$1.slow"
  if [ ! -f "data/$name/manifest.json" ]; then
    generate --seed "$2" --events "$3" --clicks "$4" --output "data/$name" > /dev/null 2>&1 || { echo "generate $name failed"; rc=1; return; }
  fi
  echo "$name: $(tr -d '\n ' < "data/$name/manifest.json" | cut -c1-70), $(ls "data/$name/events" | wc -l) event files, $(du -sm "data/$name" | cut -f1) MB"
  if [ ! -d "$exp/user_daily" ]; then
    mkdir -p "$exp"
    /usr/bin/time -f "  plain implementation: %es wall, %MKB" "$PROJ/bin/oracle" --slow "data/$name" "$exp" 2>&1 | tail -1
  fi
  bash scripts/remote/scalecheck.sh "$name" | grep -v SCALE-OK || true
  [ "${PIPESTATUS[0]}" = 0 ] || rc=1
}
large l5 "$leap" 5000000 1250000
large l20 "$leap" 20000000 5000000
large p20 31 20000000 5000000
large q60 77 60000000 15000000
[ $rc = 0 ] && echo MORESEEDS-OK || echo MORESEEDS-FAILED
exit $rc
