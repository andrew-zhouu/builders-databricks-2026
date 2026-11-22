#!/usr/bin/env bash
# Runs ON the remote machine: the 512-bit multiplications in a binary, by
# function and instruction. On Skylake-X and Cascade Lake these are the
# instructions that take the lowest clock ("licence 2"): the list says how
# much of the wide parser's hot path is of that kind. (Static counts: each of
# the decode functions runs its instructions once per eight lines.)
# Usage: mulcount.sh BINARY
bin="$1"
objdump -d -C --no-show-raw-insn "$bin" | awk '
  /^[0-9a-f]+ <.*>:$/ { fn = $2; sub(/^</, "", fn); sub(/>:$/, "", fn) }
  /%zmm/ && $2 ~ /^(vpmull[dqw]|vpmul[u]?dq|vpmulh[u]?w|vpmaddubsw|vpmaddwd|vpmadd52[lh]uq|vmulp[sd]|vfm|vfnm)/ { n[fn " " $2]++; t[fn]++ }
  /%zmm/ { z[fn]++ }
  END {
    for (f in z) printf "%-52s %5d instructions with 512-bit registers, %3d of them multiplications\n", substr(f, 1, 52), z[f], t[f]
    for (k in n) printf "    %4d  %s\n", n[k], k
  }' | sort
