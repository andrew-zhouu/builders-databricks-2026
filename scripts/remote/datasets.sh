#!/usr/bin/env bash
# Runs ON the remote machine (in tmux): generate the benchmark datasets and
# their expected answers with the plain implementation (`--slow`), which is
# itself checked against the Python reference on the small sets.
#   m5  = seed 21,  5M events   m20 = seed 20, 20M events   m60 = seed 60, 60M events
#   n20 = seed 7, 20M events (a second practice-sized set, different shape)
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
bash scripts/build_generator.sh m5:21:5000000:1250000 m20:20:20000000:5000000 n20:7:20000000:5000000 m60:60:60000000:15000000
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
cp "$BIN" "$PROJ/bin/oracle" 2>/dev/null || { mkdir -p "$PROJ/bin"; cp "$BIN" "$PROJ/bin/oracle"; }
for name in m5 m20 n20 m60; do
  exp="$PROJ/expected/$name.slow"
  if [ ! -d "$exp/user_daily" ]; then
    mkdir -p "$exp"
    /usr/bin/time -f "plain implementation $name: %es wall, %MKB" "$PROJ/bin/oracle" --slow "$PROJ/data/$name" "$exp"
  fi
  echo "expected $name: $(ls "$exp"/user_daily | wc -l) user_daily file(s)"
done
# Small sets against the independent Python reference.
bash scripts/seeds.sh 200000 50000 2 3 4 5 6 7 | tail -3
echo DATASETS-DONE
