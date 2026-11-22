#!/usr/bin/env bash
# Runs ON the remote machine: generated data sets, mutated by tools/mutate.py
# (every shape a line can take, copies that disagree, IDs of every kind, times
# out of order, gaps on the 30-minute edge, links added and removed ...), run
# through a binary under several settings and compared with the plain
# implementation (BINARY --slow), which itself is compared with the Python
# reference on the smaller sets. One line per failure; failing sets are kept
# in out/idxfuzz.
#
# Usage: idxfuzz.sh [BINARY] [COUNT] [FIRST_SEED]
#   BINARY: a path, or a name in bin/ (default: the current build)
#   env: SETTINGS=plain  thread counts, chunk sizes, scalar path, partition sizes (default)
#        SETTINGS=simd   plain, and the SIMD parser's switches (SS_SIMD=0, SS_SIMD_CHECK=1)
#        SETTINGS=split  for the block-wise splitter (branch exp-split): its self-checks
#                        (SS_SPLIT_CHECK=1, SS_SIMD_CHECK=1), SS_WSPLIT=0, SS_SIMD=0, chunk ends
#                        at and around multiples of 64; and mutations chosen for it
#        SETTINGS=index  the switches of the index back end (branch exp-index)
#        SETTINGS=wix    the index back end on the wide parser (branch exp-wix): both sets of
#                        switches, with small index partitions (SS_INDEX_PB)
#        SETTINGS=v5     for version 5 of the generator (opaque IDs, one in eight 33 bytes
#                        long): plain, the wide parser's and the splitter's switches and
#                        self-checks; and the long-ID mutations
#        SETTINGS=v5w    version 5 with the wide parser on by default: default, the base path
#                        (SS_SIMD=0), SS_WSPLIT=0, SS_WEMIT=0, both self-checks, each with thread
#                        counts, tiny chunks, chunk ends at the 64-byte marks, small partitions
#        SETTINGS=v5x    v5w, and the same again for the second wide instance (SS_SIMD=2)
#        SETTINGS=w2     the second wide instance alone (22 settings, all with SS_SIMD=2)
#        SLOW_BIN=name   the yardstick is bin/NAME --slow (default: the binary's own --slow);
#                        the binary's own --slow is then compared with it as well
#        REF_EVERY=n     the Python reference also on every n-th of the larger sets (default 5)
#        MUTATION=name   every seed with this mutation (default: by seed)
#        EVENTS=n        every seed with this many events (default: by seed)
#        ONLY=label      one setting only
#        WORK=dir        where the sets are made (default out/idxfuzz); KEEP=1 keeps them all
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="${1:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
[ -x "$BIN" ] || BIN="$PROJ/bin/$1"
[ -x "$BIN" ] || { echo "no binary $1"; exit 2; }
count="${2:-10}"; first="${3:-1}"
mode="${SETTINGS:-plain}"
PY="$PROJ/.venv/bin/python"
W="$PROJ/${WORK:-out/idxfuzz}"; mkdir -p "$W"
names=(none irregular outrange copies disorder gaps nested links wide drop irrfile badhead purchases shapes shapes
       clickshapes linkshapes shapes,clickshapes,linkshapes identityids identityids,mix shapes,copies mix mix mix mix
       emptyevents mix,shapes mix,shapes,clickshapes identityids,copies,gaps,nested userlengths endings
       shapes,userlengths,endings userlengths,mix shapes,clickshapes,linkshapes,userlengths,endings,copies
       lateness values lateness,shapes values,shapes,endings lateness,values,userlengths,clickshapes
       stretches stretches,shapes stretches,stretches,endings)
if [ "$mode" = split ]; then
  # What a block-wise line splitter has to get right, first of all.
  names=(splitter splitter,shapes splitter,endings shapes,endings splitter,stretches mix,splitter
         splitter,userlengths,values none shapes stretches,splitter,endings mix splitter,clickshapes,linkshapes
         splitter,copies,endings values,splitter mix,splitter,shapes,endings interleave interleave,shapes
         interleave,splitter,endings interleave,interleave,stretches clicksplit clicksplit,endings
         clicksplit,clickshapes,splitter clicksplit,userlengths,endings clicksplit,splitter,interleave,endings
         clicksplit,mix)
fi
if [ "$mode" = wix ]; then
  # Everything, and the splitter's shapes with the index on.
  names+=(splitter clicksplit interleave splitter,clicksplit,interleave,endings mix,splitter gaps,nested,disorder
          identityids,gaps,nested identityids,splitter,copies copies,irregular,outrange badhead,drop,links
          idbytes idbytes,copies identityids,idbytes,disorder idbytes,shapes,splitter idbytes,mix)
fi
if [ "$mode" = v5 ] || [ "$mode" = v5w ] || [ "$mode" = v5x ] || [ "$mode" = w2 ]; then
  # Version 5 of the generator: IDs are opaque, one in eight is 33 bytes long.
  # The long forms and their near misses for users, events and clicks, alone
  # and with everything else.
  names=(longusers longevents longclicks longusers,longevents,longclicks longevents,copies longusers,links,nested
         longclicks,clickshapes,clicksplit longusers,longevents,splitter,shapes longevents,idbytes,irregular
         longusers,userlengths,linkshapes mix,longusers mix,longevents,longclicks none shapes splitter clicksplit
         longusers,longevents,longclicks,splitter,clicksplit,endings,interleave stretches,longevents
         lateness,values,longevents gaps,nested,disorder,longusers wide,longusers copies drop,badhead mix mix mix
         longusers,longclicks,endings longevents,stretches,splitter,interleave idbytes,longevents,copies
         userlengths,longusers,shapes,clickshapes,linkshapes irregular,irrfile,longevents purchases,longevents,longclicks
         longusers,longusers longevents,longevents,shapes longclicks,longclicks,clicksplit mix,mix
         emptyevents,longclicks links,linkshapes,longusers values,lateness,shapes,longusers,longevents,longclicks
         windows windows,longevents,longclicks windows,longusers windows,endings,interleave windows,mix
         longusers,longevents,longclicks,windows windows,shapes,clickshapes windows,splitter,clicksplit
         windows,copies,idbytes windows,windows,stretches
         grouping longusers,longevents,longclicks,grouping windows,grouping grouping,stretches,purchases
         longusers,grouping,endings longevents,idbytes,grouping mix,grouping longclicks,clicksplit,grouping
         longlinks longlinks,endings longlinks,longusers longlinks,linkshapes,links,nested longlinks,grouping
         longlinks,mix longusers,longlinks,userlengths,endings longlinks,wide
         numbers numbers,grouping numbers,shapes,lateness numbers,longusers,longevents numbers,windows
         numbers,values,purchases,grouping numbers,mix numbers,stretches,endings)
fi
marks=(63 64 65 127 128 129 191 192 193 257 511 513 1023 4097 10000 10240 20480)
sizes=(5000 20000 60000 200000 12000 35000 9000)
# The yardstick: the plain implementation of SLOW_BIN (a name in bin/), by
# default the binary's own. With another binary's, the binary's own --slow is
# one more thing that is compared.
SLOW="$BIN"; [ -n "${SLOW_BIN:-}" ] && SLOW="$PROJ/bin/$SLOW_BIN"
[ -x "$SLOW" ] || { echo "no binary $SLOW"; exit 2; }
ref_every="${REF_EVERY:-5}"
fails=0; runs=0; indexed=0; sets=0
for seed in $(seq "$first" $((first + count - 1))); do
  events="${EVENTS:-${sizes[$((seed % ${#sizes[@]}))]}}"
  clicks=$((events / (seed % 3 + 2)))
  mutation="${MUTATION:-${names[$(((seed * 5 + seed / 7) % ${#names[@]}))]}}"
  d="$W/$seed"; rm -rf "$d"; mkdir -p "$d"
  tag="seed $seed events $events [$mutation]"
  if ! generate --seed "$seed" --events "$events" --clicks "$clicks" --output "$d/base" --threads 2 > "$d/gen.log" 2>&1; then
    echo "FAIL $tag: generator failed"; fails=$((fails + 1)); continue
  fi
  if ! "$PY" -I tools/mutate.py "$seed" "$d/base" "$d/data" "$mutation" > "$d/mutate.log" 2>&1; then
    echo "FAIL $tag: mutate.py failed: $(tail -1 "$d/mutate.log")"; fails=$((fails + 1)); continue
  fi
  rm -rf "$d/base"
  # The event key mutate.py found (the generator's): a bound forced on the
  # index path needs it given too, or the key search fails under the bound.
  key="$(grep -o 'key [0-9a-f]*' "$d/mutate.log" | head -1 | cut -d' ' -f2)"
  K=""; [ -n "$key" ] && K="SS_INDEX_KEY=$key"
  if ! timeout 300 "$SLOW" --slow "$d/data" "$d/slow" 2> "$d/slow.log"; then
    echo "FAIL $tag: the plain implementation failed: $(tail -1 "$d/slow.log")"; fails=$((fails + 1)); continue
  fi
  bad=""
  if [ "$SLOW" != "$BIN" ]; then
    runs=$((runs + 1)); mkdir -p "$d/ownslow"
    if ! timeout 300 "$BIN" --slow "$d/data" "$d/ownslow" 2> "$d/ownslow.log"; then
      echo "FAIL $tag setting ownslow (--slow): exit code $(tail -2 "$d/ownslow.log" | tr '\n' ' ' | cut -c1-300)"; bad="$bad ownslow"
    elif ! "$PY" -I tools/cmp.py "$d/slow" "$d/ownslow" > "$d/ownslow.cmp" 2>&1; then
      echo "FAIL $tag setting ownslow (--slow): $(grep -m5 -E 'rows|A:|B:' "$d/ownslow.cmp" | tr '\n' ' ' | cut -c1-400)"; bad="$bad ownslow"
    else
      rm -rf "$d/ownslow" "$d/ownslow.log" "$d/ownslow.cmp"
    fi
  fi
  # The plain implementation is the yardstick here; it has to agree with the
  # Python reference (always on the smaller sets, now and then on the others).
  if [ "$events" -le 20000 ] || [ $((seed % ref_every)) = 0 ]; then
    if ! "$PY" -I reference/reference.py "$d/data" "$d/ref" 2> "$d/ref.log"; then
      echo "FAIL $tag: the Python reference failed: $(tail -1 "$d/ref.log")"; bad="$bad reference"
    elif ! "$PY" -I tools/cmp.py "$d/ref" "$d/slow" > "$d/slow.cmp" 2>&1; then
      echo "FAIL $tag setting plain-vs-reference: $(grep -m3 -E 'rows|A:|B:' "$d/slow.cmp" | tr '\n' ' ' | cut -c1-300)"; bad="$bad plain"
    fi
  fi
  plain=(
    "default|NCPU=3"
    "one|NCPU=1"
    "eight|NCPU=8"
    "tiny|NCPU=4 SS_CHUNK_BYTES=$((seed % 97 + 31))"
    "tiny8|NCPU=8 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
    "chunk4k|NCPU=3 SS_CHUNK_BYTES=$((seed % 4096 + 2048))"
    "scalar|NCPU=2 SS_SCALAR=1"
    "manyparts|NCPU=4 SS_PD_BYTES=$((seed % 900 + 100))"
    "onepart|NCPU=5 SS_PD_BYTES=1000000000"
  )
  case "$mode" in
    plain) settings=("${plain[@]}") ;;
    v5w|v5x|w2) settings=(
      # The wide parser is the default where the processor has it, the base
      # path (SS_SIMD=0) what runs elsewhere: both count.
      "default|NCPU=3"
      "one|NCPU=1"
      "eight|NCPU=8"
      "base|NCPU=4 SS_SIMD=0"
      "base1|NCPU=1 SS_SIMD=0"
      "base8|NCPU=8 SS_SIMD=0"
      "nowsplit|NCPU=4 SS_WSPLIT=0"
      "nowemit|NCPU=4 SS_WEMIT=0"
      "nowsplitemit|NCPU=3 SS_WSPLIT=0 SS_WEMIT=0"
      "checks|NCPU=4 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1"
      "checks1|NCPU=1 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1"
      "simdcheck|NCPU=8 SS_SIMD_CHECK=1"
      "splitcheck|NCPU=2 SS_SPLIT_CHECK=1"
      "checknowsplit|NCPU=3 SS_SIMD_CHECK=1 SS_WSPLIT=0"
      "checknowemit|NCPU=3 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_WEMIT=0"
      "tiny|NCPU=4 SS_CHUNK_BYTES=$((seed % 97 + 31))"
      "tinychecks|NCPU=8 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed * 7 % 97 + 31))"
      "tinybase|NCPU=4 SS_SIMD=0 SS_CHUNK_BYTES=$((seed % 97 + 31))"
      "tiny8|NCPU=8 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
      "tiny8base|NCPU=8 SS_SIMD=0 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
      "tiny8nowemit|NCPU=8 SS_WEMIT=0 SS_CHUNK_BYTES=$((seed % 509 + 200))"
      "mark|NCPU=5 SS_CHUNK_BYTES=${marks[$(((seed / 3 + 5) % ${#marks[@]}))]}"
      "markchecks|NCPU=3 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=${marks[$((seed % ${#marks[@]}))]}"
      "markbase|NCPU=1 SS_SIMD=0 SS_CHUNK_BYTES=${marks[$(((seed / 2 + 3) % ${#marks[@]}))]}"
      "chunk4k|NCPU=3 SS_CHUNK_BYTES=$((seed % 4096 + 2048))"
      "chunk4kchecks|NCPU=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed * 3 % 4096 + 2048))"
      # Chunks of some 70 to 130 lines: a full batch of 64, then a short one.
      "chunk10k|NCPU=3 SS_CHUNK_BYTES=$((seed % 3000 + 9000))"
      "chunk10kbase|NCPU=2 SS_SIMD=0 SS_CHUNK_BYTES=$((seed * 5 % 5000 + 7000))"
      "chunk10kchecks|NCPU=4 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed * 11 % 6000 + 7000))"
      "scalar|NCPU=2 SS_SCALAR=1"
      "tinyscalar|NCPU=3 SS_SCALAR=1 SS_CHUNK_BYTES=$((seed % 97 + 31))"
      "manyparts|NCPU=4 SS_PD_BYTES=$((seed % 900 + 100))"
      "manypartsbase|NCPU=4 SS_SIMD=0 SS_PD_BYTES=$((seed % 900 + 100))"
      "onepart|NCPU=5 SS_PD_BYTES=1000000000"
      "tinyparts|NCPU=8 SS_PD_BYTES=$((seed % 300 + 64)) SS_CHUNK_BYTES=$((seed % 211 + 97))"
      "tinypartsbase|NCPU=8 SS_SIMD=0 SS_PD_BYTES=$((seed % 300 + 64)) SS_CHUNK_BYTES=$((seed % 211 + 97))")
      # v5x: also the second wide instance (SS_SIMD=2: what a processor with
      # AVX-512 but without VBMI2 runs), which is nobody's default on this box.
      # w2: that instance alone.
      w2set=(
      "w2|NCPU=3 SS_SIMD=2"
      "w2one|NCPU=1 SS_SIMD=2"
      "w2eight|NCPU=8 SS_SIMD=2"
      "w2checks|NCPU=4 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1"
      "w2checks1|NCPU=1 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1"
      "w2nowsplit|NCPU=4 SS_SIMD=2 SS_WSPLIT=0"
      "w2nowemit|NCPU=4 SS_SIMD=2 SS_WEMIT=0"
      "w2checknowsplitemit|NCPU=3 SS_SIMD=2 SS_SIMD_CHECK=1 SS_WSPLIT=0 SS_WEMIT=0"
      "w2tiny|NCPU=4 SS_SIMD=2 SS_CHUNK_BYTES=$((seed % 97 + 31))"
      "w2tinychecks|NCPU=8 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed * 7 % 97 + 31))"
      "w2tiny8|NCPU=8 SS_SIMD=2 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
      "w2mark|NCPU=5 SS_SIMD=2 SS_CHUNK_BYTES=${marks[$(((seed / 3 + 5) % ${#marks[@]}))]}"
      "w2markchecks|NCPU=3 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=${marks[$((seed % ${#marks[@]}))]}"
      "w2chunk4kchecks|NCPU=2 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed * 3 % 4096 + 2048))"
      "w2chunk10k|NCPU=3 SS_SIMD=2 SS_CHUNK_BYTES=$((seed % 3000 + 9000))"
      "w2chunk10kchecks|NCPU=4 SS_SIMD=2 SS_SIMD_CHECK=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed * 11 % 6000 + 7000))"
      "w2manyparts|NCPU=4 SS_SIMD=2 SS_PD_BYTES=$((seed % 900 + 100))"
      "w2tinyparts|NCPU=8 SS_SIMD=2 SS_PD_BYTES=$((seed % 300 + 64)) SS_CHUNK_BYTES=$((seed % 211 + 97))"
      "w2scalar|NCPU=2 SS_SIMD=2 SS_SCALAR=1"
      "w2onepart|NCPU=5 SS_SIMD=2 SS_PD_BYTES=1000000000"
      "w2simdcheck|NCPU=8 SS_SIMD=2 SS_SIMD_CHECK=1"
      "w2splitcheck|NCPU=2 SS_SIMD=2 SS_SPLIT_CHECK=1")
      [ "$mode" = v5x ] && settings+=("${w2set[@]}")
      [ "$mode" = w2 ] && settings=("${w2set[@]}") ;;
    v5) settings=("${plain[@]}"
      # (On exp-v5 the wide parser runs only with SS_SIMD=1; without it the
      # settings above are the ones that count.)
      "mark|NCPU=5 SS_CHUNK_BYTES=${marks[$(((seed / 3 + 5) % ${#marks[@]}))]}"
      "mark1|NCPU=1 SS_CHUNK_BYTES=${marks[$(((seed / 2 + 3) % ${#marks[@]}))]}"
      "tinyscalar|NCPU=3 SS_SCALAR=1 SS_CHUNK_BYTES=$((seed % 97 + 31))"
      "tinyparts|NCPU=8 SS_PD_BYTES=$((seed % 300 + 64)) SS_CHUNK_BYTES=$((seed % 211 + 97))"
      "simd|NCPU=4 SS_SIMD=1"
      "simdnowsplit|NCPU=3 SS_SIMD=1 SS_WSPLIT=0"
      "simdtiny|NCPU=8 SS_SIMD=1 SS_CHUNK_BYTES=$((seed * 7 % 97 + 31))")
      # The batch splitters' self-check, on request (SPLIT_CHECKS=1).
      [ -n "${SPLIT_CHECKS:-}" ] && settings+=(
      "simdsplit|NCPU=4 SS_SIMD=1 SS_SPLIT_CHECK=1"
      "simdsplit1|NCPU=1 SS_SIMD=1 SS_SPLIT_CHECK=1"
      "simdsplit8|NCPU=8 SS_SIMD=1 SS_SPLIT_CHECK=1 SS_PD_BYTES=$((seed % 900 + 100))"
      "simdsplittiny|NCPU=8 SS_SIMD=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed * 7 % 97 + 31))"
      "simdsplitmark|NCPU=3 SS_SIMD=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=${marks[$((seed % ${#marks[@]}))]}"
      "simdsplitchunk|NCPU=2 SS_SIMD=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed % 4096 + 2048))")
      # The wide parser's self-checks, on request (SIMD_CHECKS=1).
      [ -n "${SIMD_CHECKS:-}" ] && settings+=(
      "simdcheck|NCPU=4 SS_SIMD=1 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1"
      "simdchecknowsplit|NCPU=3 SS_SIMD=1 SS_WSPLIT=0 SS_SIMD_CHECK=1"
      "simdchecktiny|NCPU=8 SS_SIMD=1 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1 SS_CHUNK_BYTES=$((seed * 7 % 97 + 31))"
      "simdcheckmark|NCPU=3 SS_SIMD=1 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1 SS_CHUNK_BYTES=${marks[$((seed % ${#marks[@]}))]}") ;;
    simd) settings=("${plain[@]}"
      "nosimd|NCPU=4 SS_SIMD=0"
      "check|NCPU=4 SS_SIMD_CHECK=1"
      "check1|NCPU=1 SS_SIMD_CHECK=1 SS_CHUNK_BYTES=$((seed % 211 + 97))"
      "nosimd-tiny|NCPU=3 SS_SIMD=0 SS_CHUNK_BYTES=$((seed % 97 + 31))") ;;
    split) settings=(
      "default|NCPU=3"
      "one|NCPU=1"
      "eight|NCPU=8"
      "splitcheck|NCPU=4 SS_SPLIT_CHECK=1"
      "simdcheck|NCPU=4 SS_SIMD_CHECK=1"
      "bothcheck|NCPU=2 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1"
      "nowsplit|NCPU=4 SS_WSPLIT=0"
      "nosimd|NCPU=4 SS_SIMD=0"
      "tiny|NCPU=4 SS_CHUNK_BYTES=$((seed % 97 + 31))"
      "tinycheck|NCPU=8 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1 SS_CHUNK_BYTES=$((seed * 7 % 97 + 31))"
      "markcheck|NCPU=3 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1 SS_CHUNK_BYTES=${marks[$((seed % ${#marks[@]}))]}"
      "mark|NCPU=5 SS_CHUNK_BYTES=${marks[$(((seed / 3 + 5) % ${#marks[@]}))]}"
      "tiny8|NCPU=8 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
      "chunkcheck|NCPU=1 SS_SPLIT_CHECK=1 SS_CHUNK_BYTES=$((seed % 4096 + 2048))"
      "scalar|NCPU=2 SS_SCALAR=1"
      "manyparts|NCPU=4 SS_PD_BYTES=$((seed % 900 + 100))") ;;
    wix) settings=(
      "default|NCPU=3 SS_TIMING=1"
      "one|NCPU=1"
      "eight|NCPU=8"
      "checks|NCPU=4 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1"
      "noindex|NCPU=4 SS_INDEX=0"
      "nosimd|NCPU=4 SS_SIMD=0"
      "nowsplit|NCPU=3 SS_WSPLIT=0"
      "id4096|NCPU=4 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=4096"
      "id100k|NCPU=2 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=100000 SS_INDEX_PB=$((seed % 5 + 3))"
      "id50m|NCPU=8 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=50000000"
      "idcheck|NCPU=3 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=1000000 SS_INDEX_PB=$((seed % 6 + 8)) SS_SIMD_CHECK=1"
      "wrongkey|NCPU=4 SS_INDEX_KEY=123456789a"
      "sparse|NCPU=4 SS_INDEX_KEY=$((seed % 9 + 1))23456789 SS_INDEX_BOUND=15000000000 SS_INDEX_PB=18 SS_SIMD_CHECK=1"
      "halfbound|NCPU=5 $K SS_INDEX_BOUND=$((events / 2))"
      "bound64|NCPU=3 $K SS_INDEX_BOUND=64"
      "boundbig|NCPU=4 $K SS_INDEX_BOUND=200000000"
      "pb3|NCPU=4 $K SS_INDEX_PB=3"
      "pb5check|NCPU=3 $K SS_INDEX_PB=5 SS_SPLIT_CHECK=1 SS_SIMD_CHECK=1"
      "pb8half|NCPU=2 $K SS_INDEX_PB=8 SS_INDEX_BOUND=$((events * 2 / 3))"
      "pb18|NCPU=1 $K SS_INDEX_PB=18"
      "tiny|NCPU=4 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
      "tinypb|NCPU=8 $K SS_INDEX_PB=4 SS_CHUNK_BYTES=$((seed % 97 + 31))"
      "scalar|NCPU=2 SS_SCALAR=1") ;;
    index) settings=(
      "default|NCPU=3"
      "one|NCPU=1"
      "noindex|NCPU=4 SS_INDEX=0"
      "id4096|NCPU=4 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=4096"
      "id100k|NCPU=2 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=100000"
      "id50m|NCPU=8 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=50000000"
      "wrongkey|NCPU=4 SS_INDEX_KEY=123456789a"
      "halfbound|NCPU=5 SS_INDEX_BOUND=$((events / 2))"
      "bound64|NCPU=3 SS_INDEX_BOUND=64"
      "pb5|NCPU=4 SS_INDEX_PB=5"
      "idpb|NCPU=3 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=50000000 SS_INDEX_PB=$((seed % 7 + 3))"
      "tiny|NCPU=4 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
      "scalar|NCPU=2 SS_SCALAR=1") ;;
    *) echo "unknown SETTINGS=$mode"; exit 2 ;;
  esac
  for entry in "${settings[@]}"; do
    label="${entry%%|*}"; vars="${entry#*|}"
    [ -n "${ONLY:-}" ] && [ "$ONLY" != "$label" ] && continue
    o="$d/$label"; mkdir -p "$o"; runs=$((runs + 1))
    # shellcheck disable=SC2086
    env $vars timeout 300 "$BIN" "$d/data" "$o" 2> "$o.log"; rc=$?
    if [ $rc != 0 ]; then
      echo "FAIL $tag setting $label ($vars): exit code $rc $(tail -4 "$o.log" | tr '\n' ' ' | cut -c1-500)"; bad="$bad $label"; continue
    fi
    if [ "$label" = default ]; then
      sets=$((sets + 1)); grep -q 'event IDs indexed: true' "$o.log" && indexed=$((indexed + 1))
    fi
    if ! "$PY" -I tools/cmp.py "$d/slow" "$o" > "$o.cmp" 2>&1; then
      echo "FAIL $tag setting $label ($vars): $(grep -m5 -E 'rows|A:|B:' "$o.cmp" | tr '\n' ' ' | cut -c1-400)"; bad="$bad $label"
    else
      rm -rf "$o" "$o.log" "$o.cmp"
    fi
  done
  if [ -n "$bad" ]; then
    fails=$((fails + 1)); echo "     kept $d  ($(cut -c1-200 "$d/mutate.log"))"
  elif [ -z "${KEEP:-}" ]; then
    rm -rf "$d"
  fi
done
note=""; [ "$mode" = wix ] && note=" ($indexed of $sets sets took the index road by default)"
echo "idxfuzz $(basename "$BIN") [$mode] seeds $first..$((first + count - 1)): $runs runs, $fails failing sets$note"
[ "$fails" = 0 ]
