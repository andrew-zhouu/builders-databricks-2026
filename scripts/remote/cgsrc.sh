#!/usr/bin/env bash
# Runs ON the remote machine: profile the current sources with valgrind (as
# cgasm.sh) and list one function's instructions per event row by source line,
# for the source files matching a regex, with their total.
# Usage: cgsrc.sh FUNCTION_REGEX FILE_REGEX [MAX_LINES] [DATASET]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
fn="$1"; files="$2"; max="${3:-40}"; name="${4:-dev}"
if [ "${REUSE:-0}" != 1 ]; then bash scripts/remote/cgasm.sh "$fn" "$name" 0.001 > /tmp/ss-cgsrc.txt 2>&1; else
  rows=$(cat "data/$name"/events/*.csv | wc -l)
  python3 -I "$PROJ/tools/cgasm.py" /tmp/ss-cgasm/cg.out /tmp/ss-cgasm/asm.txt "$rows" "$fn" 0.001 > /tmp/ss-cgsrc.txt 2>&1
fi
head -1 /tmp/ss-cgsrc.txt
sed -n '/^-- instructions/,$p' /tmp/ss-cgsrc.txt | awk -v re="$files" '
  NR > 1 { src = $NF; if (src ~ re) { sum[src] += $1; total += $1 } }
  END { for (s in sum) printf "%9.2f  %s\n", sum[s], s; printf "%9.2f  TOTAL for files matching %s\n", total, re }' | sort -rn | head -"$max"
