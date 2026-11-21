#!/usr/bin/env bash
# User-mode cycles per row for each section, per variant (min over ROUNDS,
# interleaved). User cycles exclude kernel time, so this isolates what the
# code itself costs, memory stalls included.
# Usage: cyc.sh DATASET NCPU ROUNDS VARIANT...     (variants as in sysvar.sh)
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
BIN="${BIN:-$PROJ/fast/target/x86_64-unknown-linux-musl/release/sessions}"
name="$1"; ncpu="$2"; rounds="$3"; shift 3
out="/tmp/ss-cyc-$$"; res="/tmp/ss-cyc-$$.res"; : > "$res"
for r in $(seq "$rounds"); do
  for v in "$@"; do
    envs="${v//,/ }"; bin="$BIN"
    for kv in $envs; do case "$kv" in SSBIN=*) bin="$PROJ/bin/${kv#SSBIN=}";; esac; done
    rm -rf "$out"; mkdir -p "$out"
    env $envs NCPU="$ncpu" SS_TIMING=1 ${PIN:+taskset -c $PIN} "$bin" "$PROJ/data/$name" "$out" 2>"$out.log"
    # name is two words for most sections; fields from the right: llc brmiss ipc cycles instr rows faults sys % cpu
    awk -v v="$v" '/^    (parse|dedup|sessions|links|write)/ { n=NF; sec=$1; if ($2 !~ /^[0-9.]+$/) sec=sec"_"$2;
        print v, sec, $(n-3), $(n-4), $(n-5), $(n-1), $(n) }
      /sections: cpu/ { n=split($0,a,/[ ,]+/); for(i=1;i<=n;i++) if (a[i]=="G" && a[i+1]=="user") print v, "TOTAL_Gcycles", a[i-1]*1000, a[i-4]*1000, 1, 0, 0 }' "$out.log" >> "$res"
  done
done
rm -rf "$out" "$out.log"
secs=$(awk '{print $2}' "$res" | awk '!s[$0]++')
printf '%-22s' "section (cycles/row)"; for v in "$@"; do printf ' %22s' "${v:0:22}"; done; echo
for s in $secs; do
  printf '%-22s' "$s"
  for v in "$@"; do
    awk -v v="$v" -v s="$s" '$1==v && $2==s {print $3, $4, $6, $7}' "$res" | sort -n | head -1 | awk '{printf " %8.1f (i%5.0f m%4.2f)", $1, $2, $4}'
  done
  echo
done
rm -f "$res"
echo "(min cycles/row over $rounds rounds; i = instructions/row, m = LLC misses/row; TOTAL in millions; ncpu=$ncpu; load $(cut -d' ' -f1 /proc/loadavg))"
