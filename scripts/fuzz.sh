#!/usr/bin/env bash
# Differential fuzzing: adversarial datasets, binary (fast, scalar-only, plain)
# against the Python reference. Usage: fuzz.sh FIRST_SEED COUNT [ROWS]
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
PY="$PROJ/.venv/bin/python"
first="$1"; count="$2"; rows="${3:-4000}"
W="${FUZZ_DIR:-/tmp/ss-fuzz-$$}"; mkdir -p "$W"
fails=0
for seed in $(seq "$first" $((first + count - 1))); do
  d="$W/$seed"; rm -rf "$d"; mkdir -p "$d"
  "$PY" -I "$PROJ/tools/fuzz.py" "$seed" "$d/data" "$rows" || { echo "seed $seed: generator failed"; fails=$((fails+1)); continue; }
  if ! "$PY" -I "$PROJ/reference/reference.py" "$d/data" "$d/expected" 2>"$d/ref.log"; then
    echo "seed $seed: reference failed: $(tail -1 "$d/ref.log")"; fails=$((fails+1)); continue
  fi
  bad=""
  for mode in fast scalar slow one tiny tiny8 read readtiny readscalar readtail; do
    o="$d/$mode"; mkdir -p "$o"
    case "$mode" in
      fast)   NCPU=3 "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      scalar) NCPU=2 SCALAR=1 "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      slow)   "$BIN" --slow "$d/data" "$o" 2>"$o.log" ;;
      one)    NCPU=1 "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      # tiny chunks: lines of every kind start, end and straddle chunk boundaries
      tiny)   NCPU=4 CHUNK_BYTES=$((seed % 97 + 31)) "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      tiny8)  NCPU=8 CHUNK_BYTES=$((seed % 1013 + 300)) "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      # the pread input mode, whole files and tiny chunks
      read)       NCPU=3 IO=read "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      readtiny)   NCPU=4 IO=read CHUNK_BYTES=$((seed % 211 + 17)) "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      readscalar) NCPU=2 IO=read SCALAR=1 CHUNK_BYTES=$((seed % 5000 + 100)) "$BIN" "$d/data" "$o" 2>"$o.log" ;;
      # a tail too short for most lines: every chunk has to extend its read
      readtail)   NCPU=3 IO=read READ_TAIL=$((seed % 7)) CHUNK_BYTES=$((seed % 401 + 50)) "$BIN" "$d/data" "$o" 2>"$o.log" ;;
    esac
    if [ $? -ne 0 ] || ! "$PY" -I "$PROJ/tools/cmp.py" "$d/expected" "$o" >"$o.cmp" 2>&1; then
      bad="$bad $mode"
    fi
  done
  if [ -n "$bad" ]; then
    echo "seed $seed: MISMATCH in:$bad  (kept $d)"; fails=$((fails+1))
  else
    rm -rf "$d"
  fi
done
echo "fuzz seeds $first..$((first + count - 1)) rows=$rows: $fails failing"
[ "$fails" = 0 ]
