#!/usr/bin/env bash
# Runs ON the remote machine: parser instance 3 (four lanes in 256-bit
# registers, for processors that lower their clock for 512-bit instructions
# and for 256-bit multiplications) must hold neither. The compiler, with
# AVX-512 enabled for a function, makes 512-bit instructions of pairs of
# 256-bit ones and multiplications of sums of shifts on its own, so the
# binary is what is looked at: every function of the instance (those of
# parse::wide::quad and parse::splitw::quad, and the chunk functions named
# ..._quad where they are not part of their caller; those are ordinary
# functions, compiled without AVX-512).
# Prints what it finds; exits 1 on a 512-bit register or a multiplication on a
# 256-bit one there, or if the instance is not in the binary at all.
# Usage: quadcheck.sh BINARY
bin="$1"
objdump -d -C --no-show-raw-insn "$bin" | awk '
  /^[0-9a-f]+ <.*>:$/ { fn = $2; sub(/^</, "", fn); sub(/>:$/, "", fn); on = (fn ~ /parse::wide::quad::|parse::splitw::quad::|_chunk_quad/); if (on) seen[fn] = 1; next }
  !on { next }
  /%ymm/ { y[fn]++ }
  /%k[0-7]/ { k[fn]++ }
  /%zmm/ { bad[fn " " $2 " (512-bit register)"]++ }
  /%ymm/ && $2 ~ /^(vpmull[dqw]|vpmul[u]?dq|vpmulh[u]?w|vpmulhrsw|vpmaddubsw|vpmaddwd|vpmadd52[lh]uq|vmulp[sd]|vfm|vfnm|vdivp|vsqrtp)/ { bad[fn " " $2 " (256-bit multiplication)"]++ }
  END {
    for (f in seen) printf "%-50s %4d instructions with 256-bit registers, %4d with mask registers\n", substr(f, 1, 50), y[f], k[f]
    for (b in bad) printf "BAD %4d  %s\n", bad[b], b
  }' | sort > /tmp/ss-quadcheck.txt
cat /tmp/ss-quadcheck.txt
grep -q -v '^BAD' /tmp/ss-quadcheck.txt || { echo "instance 3 is not in this binary"; exit 1; }
if grep -q '^BAD' /tmp/ss-quadcheck.txt; then echo "INSTANCE 3 HOLDS WHAT IT MUST NOT"; exit 1; fi
echo "instance 3: no 512-bit register and no 256-bit multiplication in $(grep -c -v '^BAD' /tmp/ss-quadcheck.txt) functions"
