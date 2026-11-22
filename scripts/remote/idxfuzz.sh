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
#        SETTINGS=index  the switches of the index back end (branch exp-index)
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
       emptyevents mix,shapes mix,shapes,clickshapes identityids,copies,gaps,nested)
sizes=(5000 20000 60000 200000 12000 35000 9000)
fails=0; runs=0
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
      echo "FAIL $tag setting $label ($vars): exit code $rc $(tail -1 "$o.log" | cut -c1-200)"; bad="$bad $label"; continue
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
echo "idxfuzz $(basename "$BIN") [$mode] seeds $first..$((first + count - 1)): $runs runs, $fails failing sets"
[ "$fails" = 0 ]
