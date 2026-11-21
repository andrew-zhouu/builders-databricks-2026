#!/usr/bin/env bash
# Runs ON the remote machine (needs sudo): where does the time of one run go,
# kernel included? Samples every CPU with call stacks for the length of the run.
# Usage: kprof.sh [DATASET] [BINARY_NAME_IN_bin_OR_empty] [extra env assignments...]
. "$(dirname "${BASH_SOURCE[0]}")/../../scripts/env.sh"
cd "$PROJ"
name="${1:-m20}"; which="${2:-}"; shift 2 2>/dev/null || true
BIN="$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions"
[ -n "$which" ] && BIN="$PROJ/bin/$which"
out=/tmp/ss-kprof; rm -rf "$out"; mkdir -p "$out"
cat data/"$name"/*/*.csv > /dev/null
sudo perf record -q -F 2999 -g -a -o /tmp/ss-perf.data -- env NCPU=8 "$@" "$BIN" "data/$name" "$out" 2>/dev/null
sudo chown "$USER" /tmp/ss-perf.data
echo "== by command and privilege level"
perf report -i /tmp/ss-perf.data --stdio --no-children --sort comm 2>/dev/null | grep -v '^#' | grep -v '^$' | head -6
echo "== top symbols (self time)"
perf report -i /tmp/ss-perf.data --stdio --no-children -g none --sort dso,sym 2>/dev/null | grep -v '^#' | grep -v '^$' | head -"${TOP:-45}" | cut -c1-140
echo "== kernel entry points (callers of the hot kernel symbols)"
perf report -i /tmp/ss-perf.data --stdio --children -g none --sort sym 2>/dev/null | grep -E 'asm_exc_page_fault|do_syscall_64|__x64_sys_|do_user_addr_fault|handle_mm_fault|exit_mmap|do_exit|x64_sys_call' | head -25 | cut -c1-140
rm -rf "$out"
