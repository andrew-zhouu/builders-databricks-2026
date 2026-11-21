#!/usr/bin/env bash
# Runs ON the remote machine: what hardware, kernel and tools does it have?
uname -srm
echo "cpus: $(nproc)"
lscpu | grep -E 'Model name|Thread\(s\)|Core\(s\)|Socket|L1d|L2|L3|Hypervisor|MHz'
echo "avx512: $(grep -o 'avx512[a-z0-9_]*' /proc/cpuinfo | sort -u | tr '\n' ' ')"
for c in 0 1 2 3 4 5 6 7; do printf 'cpu%s siblings %s; ' "$c" "$(cat /sys/devices/system/cpu/cpu$c/topology/thread_siblings_list 2>/dev/null)"; done; echo
free -g | sed -n 2p
df -h / /dev/shm | tail -2
lsblk -o NAME,SIZE,TYPE,MOUNTPOINT | head -8
echo "THP: $(cat /sys/kernel/mm/transparent_hugepage/enabled) defrag: $(cat /sys/kernel/mm/transparent_hugepage/defrag)"
echo "perf_event_paranoid: $(cat /proc/sys/kernel/perf_event_paranoid)"
for t in docker cargo rustc perf tmux git python3 rsync unzip gcc; do printf '%s=%s ' "$t" "$(command -v $t || echo MISSING)"; done; echo
sudo -n true 2>/dev/null && echo "sudo: passwordless" || echo "sudo: needs password"
uptime
ps -eo pcpu,rss,comm --sort=-pcpu | head -4
