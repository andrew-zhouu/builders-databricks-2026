#!/usr/bin/env bash
# Runs ON the remote machine (under the lock; needs sudo): wall time of the
# given variants with the kernel's fault-around as it is, and with it turned
# off (one page mapped per fault: what a file system or kernel without it
# gives), which is put back afterwards.
# Usage: faultaround.sh DATASET ROUNDS VARIANT...
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="$1"; rounds="$2"; shift 2
F=/sys/kernel/debug/fault_around_bytes
was=$(sudo cat "$F" 2>/dev/null)
echo "fault_around_bytes: ${was:-unreadable}"
bash scripts/abx.sh "$name" "$rounds" "$@" 2>&1 | tail -$(( $# + 2 ))
if [ -n "$was" ]; then
  echo 4096 | sudo tee "$F" > /dev/null
  echo "fault_around_bytes now: $(sudo cat "$F")"
  bash scripts/abx.sh "$name" "$rounds" "$@" 2>&1 | tail -$(( $# + 2 ))
  for v in "$@"; do
    o=/tmp/ss-fa-$$; rm -rf "$o"
    env ${v//,/ } NCPU=8 SS_TIMING=1 fast/target/x86_64-unknown-linux-musl/release/sessions "data/$name" "$o" 2>&1 | grep -E 'parse events|parse clicks|process:' | cut -c1-150
    rm -rf "$o"
  done
  echo "$was" | sudo tee "$F" > /dev/null
  echo "fault_around_bytes restored: $(sudo cat "$F")"
fi
