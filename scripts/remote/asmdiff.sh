#!/usr/bin/env bash
# Runs ON the remote machine: are functions the same instructions in two
# binaries? For each symbol pattern, the function's disassembly in both, with
# what depends on where the code lies taken out (addresses, distances to
# constants, the addresses in front of jump and call targets), compared.
# Prints the number of instructions and of differing lines; exits 1 if any
# function differs or is missing.
# Usage: asmdiff.sh BINARY_A BINARY_B SYMBOL_REGEX...     (a regex on "<name>:", e.g. 'parse::wide::decode8>')
a="$1"; b="$2"; shift 2
norm() {
  objdump -d -C --no-show-raw-insn "$1" | awk -v re="$2" '
    /^[0-9a-f]+ <.*>:$/ { on = ($0 ~ re); next }
    on && NF {
      sub(/^ *[0-9a-f]+:[ \t]*/, "")
      gsub(/-?0x[0-9a-f]+\(%rip\)/, "X(%rip)")
      sub(/ *# .*$/, "")
      gsub(/[0-9a-f]+ </, "<")
      print
    }'
}
rc=0
for re in "$@"; do
  norm "$a" "$re" > /tmp/ss-asmdiff-a.txt
  norm "$b" "$re" > /tmp/ss-asmdiff-b.txt
  na=$(wc -l < /tmp/ss-asmdiff-a.txt); nb=$(wc -l < /tmp/ss-asmdiff-b.txt)
  d=$(diff /tmp/ss-asmdiff-a.txt /tmp/ss-asmdiff-b.txt | grep -c '^[<>]')
  if [ "$na" -gt 0 ] && [ "$d" = 0 ]; then
    echo "same   $re ($na instructions)"
  else
    echo "DIFFER $re ($na and $nb instructions, $d differing lines)"; rc=1
    diff /tmp/ss-asmdiff-a.txt /tmp/ss-asmdiff-b.txt | head -8 | cut -c1-120
  fi
done
rm -f /tmp/ss-asmdiff-a.txt /tmp/ss-asmdiff-b.txt
exit $rc
