#!/usr/bin/env bash
# Runs ON the remote machine: the 512-bit multiplications in a binary, by
# function and instruction. On Skylake-X and Cascade Lake these are the
# instructions that take the lowest clock ("licence 2"): the list says how
# much of the wide parser's hot path is of that kind. (Static counts: each of
# the decode functions runs its instructions once per eight lines.)
#
# The parser instance for those processors (SS_SIMD=2: the functions named
# ..._wide5 and those of parse::wide::half) must have none: exits 1 if it has.
# Usage: mulcount.sh BINARY
bin="$1"
objdump -d -C --no-show-raw-insn "$bin" | awk '
  /^[0-9a-f]+ <.*>:$/ { fn = $2; sub(/^</, "", fn); sub(/>:$/, "", fn) }
  /%zmm/ && $2 ~ /^(vpmull[dqw]|vpmul[u]?dq|vpmulh[u]?w|vpmulhrsw|vpmaddubsw|vpmaddwd|vpmadd52[lh]uq|vmulp[sd]|vfm|vfnm)/ { n[fn " " $2]++; t[fn]++ }
  /%zmm/ { z[fn]++ }
  END {
    for (f in z) printf "%-58s %5d instructions with 512-bit registers, %3d of them multiplications\n", substr(f, 1, 58), z[f], t[f]
    for (k in n) printf "    %4d  %s\n", n[k], k
  }' | sort > /tmp/ss-mulcount.txt
cat /tmp/ss-mulcount.txt
if grep -E '^ +[0-9]+  .*(_wide5|parse::wide::half::)' /tmp/ss-mulcount.txt | grep -q .; then
  echo "512-BIT MULTIPLICATIONS in the instance that must have none:"
  grep -E '^ +[0-9]+  .*(_wide5|parse::wide::half::)' /tmp/ss-mulcount.txt
  exit 1
fi
echo "no 512-bit multiplication in the instance for processors without VBMI2 ($(grep -c -E '(_wide5|parse::wide::half::)' /tmp/ss-mulcount.txt) functions of it with 512-bit registers)"
