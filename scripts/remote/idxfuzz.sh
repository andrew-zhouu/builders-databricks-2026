#!/usr/bin/env bash
# Runs ON the remote machine: generated data sets, mutated by tools/mutate.py
# to lean on everything an index-based back end could get wrong, run through a
# binary under several settings and compared with the plain implementation
# (BINARY --slow), which itself is compared with the Python reference on the
# smaller sets. One line per failure; failing sets are kept in out/idxfuzz.
#
# Usage: idxfuzz.sh [BINARY] [COUNT] [FIRST_SEED]
#   BINARY: a path, or a name in bin/ (default: the current build)
#   env: MUTATION=name   every seed with this mutation (default: by seed)
#        EVENTS=n        every seed with this many events (default: by seed)
#        ONLY=label      one setting only
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
BIN="${1:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
[ -x "$BIN" ] || BIN="$PROJ/bin/$1"
[ -x "$BIN" ] || { echo "no binary $1"; exit 2; }
count="${2:-10}"; first="${3:-1}"
PY="$PROJ/.venv/bin/python"
W="$PROJ/out/idxfuzz"; mkdir -p "$W"
names=(none irregular outrange copies disorder gaps links wide drop irrfile badhead purchases mix mix mix mix emptyevents mix)
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
  settings=(
    "default|NCPU=3"
    "one|NCPU=1"
    "noindex|NCPU=4 SS_INDEX=0"
    "id4096|NCPU=4 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=4096"
    "id100k|NCPU=2 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=100000"
    "id50m|NCPU=8 SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=50000000"
    "wrongkey|NCPU=4 SS_INDEX_KEY=123456789a"
    "halfbound|NCPU=5 SS_INDEX_BOUND=$((events / 2))"
    "bound64|NCPU=3 SS_INDEX_BOUND=64"
    "tiny|NCPU=4 SS_CHUNK_BYTES=$((seed % 1013 + 300))"
    "scalar|NCPU=2 SS_SCALAR=1"
  )
  for entry in "${settings[@]}"; do
    label="${entry%%|*}"; vars="${entry#*|}"
    [ -n "${ONLY:-}" ] && [ "$ONLY" != "$label" ] && continue
    o="$d/$label"; mkdir -p "$o"; runs=$((runs + 1))
    # shellcheck disable=SC2086
    if ! env $vars timeout 300 "$BIN" "$d/data" "$o" 2> "$o.log"; then
      echo "FAIL $tag setting $label ($vars): exit code $? $(tail -1 "$o.log" | cut -c1-200)"; bad="$bad $label"; continue
    fi
    if ! "$PY" -I tools/cmp.py "$d/slow" "$o" > "$o.cmp" 2>&1; then
      echo "FAIL $tag setting $label ($vars): $(grep -m5 -E 'rows|A:|B:' "$o.cmp" | tr '\n' ' ' | cut -c1-400)"; bad="$bad $label"
    else
      rm -rf "$o" "$o.log" "$o.cmp"
    fi
  done
  if [ -n "$bad" ]; then
    fails=$((fails + 1)); echo "     kept $d  ($(cat "$d/mutate.log" | cut -c1-200))"
  else
    rm -rf "$d"
  fi
done
echo "idxfuzz $(basename "$BIN") seeds $first..$((first + count - 1)): $runs runs, $fails failing sets"
[ "$fails" = 0 ]
