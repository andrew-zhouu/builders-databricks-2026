#!/usr/bin/env bash
# Runs ON the remote machine: one stage of the battery for a binary with the
# index path (each stage is a few minutes, so that the machine's lock changes
# hands in between).
#   idxbattery.sh BINARY_NAME basics             fixtures, dev answer, six generated seeds against the Python reference
#   idxbattery.sh BINARY_NAME mutate FIRST COUNT mutated generated sets (idxfuzz.sh)
#   idxbattery.sh BINARY_NAME dense FIRST COUNT  the adversarial corpus with dense IDs (fuzzidx.sh), and with its usual IDs
#   idxbattery.sh BINARY_NAME fuzz FIRST COUNT   the usual fuzz battery (scripts/fuzz.sh), plain and with the index switches
#   idxbattery.sh BINARY_NAME scale [SETS...]    the large sets against their reference answers
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; stage="$2"; shift 2
export BIN="$PROJ/bin/$name"
[ -x "$BIN" ] || { echo "no binary bin/$name"; exit 2; }
case "$stage" in
  basics)
    bash scripts/check.sh 2>&1 | tail -3
    NCPU=8 bash scripts/seeds.sh 200000 50000 2 3 4 5 6 7 2>&1 | grep -v '^ok' | tail -12
    echo "seeds done ($(NCPU=8 bash scripts/seeds.sh 200000 50000 2 3 4 5 6 7 2>&1 | grep -c '^ok') ok lines of 12)" ;;
  mutate)
    bash scripts/remote/idxfuzz.sh "$BIN" "${2:-12}" "${1:-1}" 2>&1 | cut -c1-420 ;;
  dense)
    bash scripts/remote/fuzzidx.sh "$BIN" "${2:-10}" "${1:-1}" 3000 2>&1 | cut -c1-420
    FUZZ_IDS=plain bash scripts/remote/fuzzidx.sh "$BIN" "$(( (${2:-10} + 1) / 2 ))" "${1:-1}" 3000 2>&1 | cut -c1-420 ;;
  fuzz)
    bash scripts/fuzz.sh "${1:-1}" "${2:-20}" 3000 2>&1 | tail -4 | cut -c1-300
    SS_INDEX_IDENTITY=1 SS_INDEX_BOUND=50000000 SS_INDEX_PB=4 FUZZ_IDS=dense bash scripts/fuzz.sh "${1:-1}" "${2:-20}" 3000 2>&1 | tail -4 | cut -c1-300 ;;
  scale)
    bash scripts/remote/scalecheck.sh "$@" 2>&1 | cut -c1-300 ;;
  *) echo "unknown stage $stage"; exit 2 ;;
esac
