#!/usr/bin/env bash
# Runs ON the remote machine: the annotated instructions of one function from
# the last cgasm.sh profile (no source-line summary).
# Usage: cgshow.sh FUNCTION_REGEX [MIN_PER_ROW] [MAX_LINES]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
fn="$1"; min="${2:-0.02}"; max="${3:-200}"
out=/tmp/ss-cgasm
rows=$(cat "data/${DATASET:-dev}"/events/*.csv | wc -l)
python3 -I "$PROJ/tools/cgasm.py" "$out/cg.out" "$out/asm.txt" "$rows" "$fn" "$min" > "$out/show.txt" 2>/dev/null
head -1 "$out/show.txt"
sed -n '/^-- instructions/,$p' "$out/show.txt" | sed 's/<sessions::[^>]*>//; s/lib:core\/src\/..\/..\/stdarch\/crates\/core_arch\/src\//arch:/' | cut -c1-104 | head -"$max"
