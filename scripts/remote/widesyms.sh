#!/usr/bin/env bash
# Runs ON the remote machine: which functions of a binary contain AVX-512
# instructions (512-bit registers or mask registers)? They may only be the
# ones behind the run-time test for an Ice Lake or newer processor: the wide
# parser instance (parse::wide, events_chunk_wide and the like) and the batch
# splitter (parse::splitw). On any other processor those are never called,
# and 512-bit code anywhere else would lower a Cascade Lake core's clock, or
# not run at all on a processor without AVX-512.
# Prints the functions and exits 1 if one is not on that list.
# Usage: widesyms.sh BINARY
bin="$1"
objdump -d -C --no-show-raw-insn "$bin" | awk '
  /^[0-9a-f]+ <.*>:$/ { fn = $2; sub(/^</, "", fn); sub(/>:$/, "", fn) }
  /%zmm|%k[0-7]/ { n[fn]++ }
  END { for (f in n) printf "%6d  %s\n", n[f], f }' | sort -k2 > /tmp/ss-widesyms.txt
allowed='parse::wide::|parse::splitw::|events_chunk_wide|clicks_chunk_wide|split_event_wide|split_click_wide'
echo "functions with AVX-512 instructions: $(wc -l < /tmp/ss-widesyms.txt)"
cut -c1-110 /tmp/ss-widesyms.txt
if grep -v -E "$allowed" /tmp/ss-widesyms.txt | grep -q .; then
  echo "NOT behind the run-time test:"; grep -v -E "$allowed" /tmp/ss-widesyms.txt | cut -c1-140
  exit 1
fi
