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
#        MUTATION=name   every seed with this mutation (default: by seed)
#        EVENTS=n        every seed with this many events (default: by seed)
#        ONLY=label      one setting only
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="${1:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
[ -x "$BIN" ] || BIN="$PROJ/bin/$1"
[ -x "$BIN" ] || { echo "no binary $1"; exit 2; }
count="${2:-10}"; first="${3:-1}"
mode="${SETTINGS:-plain}"
PY="$PROJ/.venv/bin/python"
W="$PROJ/out/idxfuzz"; mkdir -p "$W"
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
marks=(63 64 65 127 128 129 191 192 193 257 511 513 1023 4097 10000 10240 20480)
sizes=(5000 20000 60000 200000 12000 35000 9000)
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
  if ! timeout 300 "$BIN" --slow "$d/data" "$d/slow" 2> "$d/slow.log"; then
    echo "FAIL $tag: the plain implementation failed: $(tail -1 "$d/slow.log")"; fails=$((fails + 1)); continue
  fi
  bad=""
  # The plain implementation is the yardstick here; it has to agree with the
  # Python reference (always on the smaller sets, now and then on the others).
  if [ "$events" -le 20000 ] || [ $((seed % 5)) = 0 ]; then
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
  else
    rm -rf "$d"
  fi
done
note=""; [ "$mode" = wix ] && note=" ($indexed of $sets sets took the index road by default)"
echo "idxfuzz $(basename "$BIN") [$mode] seeds $first..$((first + count - 1)): $runs runs, $fails failing sets$note"
[ "$fails" = 0 ]
