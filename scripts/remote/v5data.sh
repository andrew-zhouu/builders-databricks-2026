#!/usr/bin/env bash
# Runs ON the remote machine (in tmux, under the lock): move to the materials
# of scorer version 5. The old official files, data sets and answers are kept
# as official-v4, data-v4, expected-v4; the new generator is built and the sets
# dev, m5, m20 (then n20, m60) are generated again under their old names, with
# their answers from the plain implementation of the kept binary bin/oracle,
# once that has reproduced the new official dev answer.
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
df -h "$PROJ" | tail -1
if [ ! -d official-v4 ]; then
  mv official official-v4 && mv data data-v4 && mv expected expected-v4 && mkdir -p data expected
  mv .toolchain/bin/generate .toolchain/bin/generate-v4
  rm -rf .toolchain/gen-target
fi
if [ ! -f official/generator/x/generator/src/text.rs ]; then bash scripts/fetch_official.sh; fi
echo "Feistel in the generator: $(grep -c Feistel official/generator/x/generator/src/text.rs); scorer: $(grep -o 'shopping-sessions-[0-9]*' official/page/page.html | sort -u | tr '\n' ' ')"
bash scripts/build_generator.sh dev:1:1000000:250000 2>&1 | tail -5
PY="$PROJ/.venv/bin/python"
ok=1
for mode in "" "--slow"; do
  out="out/v5-dev$mode"; rm -rf "$out"; mkdir -p "$out"
  s=$(date +%s%N)
  if NCPU=8 bin/oracle $mode data/dev "$out" 2> "$out.log" && "$PY" -I tools/cmp.py official/dev-answer/x/dev-answer "$out" > "$out.cmp" 2>&1; then
    echo "ok   oracle $mode on the new dev set = new official dev answer ($(( ($(date +%s%N) - s) / 1000000 )) ms)"
  else
    echo "FAIL oracle $mode on the new dev set"; tail -3 "$out.log"; head -8 "$out.cmp"; ok=0
  fi
done
head -3 data/dev/events/*.csv | cut -c1-160 | head -8
bash scripts/build_generator.sh m5:21:5000000:1250000 m20:20:20000000:5000000 2>&1 | grep -v '^  '
for name in m5 m20; do
  exp="expected/$name.slow"
  if [ ! -d "$exp/user_daily" ]; then
    mkdir -p "$exp"
    /usr/bin/time -f "plain implementation $name: %es wall, %MKB" bin/oracle --slow "data/$name" "$exp"
  fi
done
echo "V5-FIRST-DONE (oracle agrees with the official dev answer: $ok)"
# What the old binaries make of the new data.
for b in tbl c8; do
  for name in m5 m20; do
    o="out/v5-$b-$name"; rm -rf "$o"; mkdir -p "$o"
    s=$(date +%s%N)
    NCPU=8 "bin/$b" "data/$name" "$o" 2> "$o.log"; rc=$?
    ms=$(( ($(date +%s%N) - s) / 1000000 ))
    "$PY" -I tools/cmp.py "expected/$name.slow" "$o" > "$o.cmp" 2>&1 && same=identical || same=DIFFERENT
    echo "bin/$b on $name: $ms ms, exit $rc, $same"
    rm -rf "$o"
  done
done
NCPU=8 SS_TIMING=1 bin/c8 data/m20 out/v5-timing 2>&1 | cut -c1-175 | tail -24; rm -rf out/v5-timing
echo V5-TIMED
bash scripts/build_generator.sh n20:7:20000000:5000000 m60:60:60000000:15000000 2>&1 | grep -v '^  '
for name in n20 m60; do
  exp="expected/$name.slow"
  if [ ! -d "$exp/user_daily" ]; then
    mkdir -p "$exp"
    /usr/bin/time -f "plain implementation $name: %es wall, %MKB" bin/oracle --slow "data/$name" "$exp"
  fi
done
bash scripts/seeds.sh 200000 50000 2 3 4 5 6 7 2>&1 | tail -12
echo V5-DATA-DONE
