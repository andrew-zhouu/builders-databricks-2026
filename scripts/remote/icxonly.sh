#!/usr/bin/env bash
# Runs ON the remote machine: which functions of a binary contain instructions
# that Skylake-X and Cascade Lake do not have (AVX-512 VBMI, VBMI2, VPOPCNTDQ,
# BITALG, VNNI, IFMA, GFNI, VAES, wide VPCLMULQDQ)? Only the parser instance
# for processors with VBMI2 may: its batch splitters and its two chunk
# functions (the splitters may be inlined there). Anything else would be an
# illegal instruction on a processor that takes the instance without VBMI2.
# Prints the functions and exits 1 if one is not on that list.
# Usage: icxonly.sh BINARY
bin="$1"
objdump -d -C --no-show-raw-insn "$bin" | awk '
  /^[0-9a-f]+ <.*>:$/ { fn = $2; sub(/^</, "", fn); sub(/>:$/, "", fn) }
  $2 ~ /^(vpcompress[bw]|vpexpand[bw]|vpshld|vpshrd|vpermb|vpermi2b|vpermt2b|vpmultishiftqb|vpopcnt[bwdq]|vpshufbitqmb|vpdpbus|vpdpwss|vpmadd52|vgf2p8|gf2p8|vaes|vp2intersect)/ { n[fn " " $2]++ }
  $2 ~ /^vpclmulqdq/ && /%[yz]mm/ { n[fn " " $2]++ }
  END { for (k in n) printf "%6d  %s\n", n[k], k }' | sort -k2 > /tmp/ss-icxonly.txt
echo "instructions newer than Cascade Lake, by function:"
cut -c1-120 /tmp/ss-icxonly.txt
allowed='parse::splitw::split_batch |parse::splitw::split_clicks_batch |parse::events_chunk_wide |parse::clicks_chunk_wide |parse::splitw::scan |parse::splitw::lowest |parse::splitw::ones '
if grep -v -E "$allowed" /tmp/ss-icxonly.txt | grep -q .; then
  echo "OUTSIDE the instance for processors with VBMI2:"; grep -v -E "$allowed" /tmp/ss-icxonly.txt | cut -c1-140
  exit 1
fi
echo "only in the instance for processors with VBMI2 ($(wc -l < /tmp/ss-icxonly.txt) function/instruction pairs)"
