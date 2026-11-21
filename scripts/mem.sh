#!/usr/bin/env bash
# Where is the memory? Top RSS processes and kernel counters.
free -m | head -2
ps -eo pid,rss,pcpu,etimes,args --sort=-rss | head -8 | cut -c1-150
grep -E 'MemTotal|MemFree|MemAvailable|^Cached|Buffers|AnonPages|AnonHugePages|Shmem:|Slab|SReclaimable|SUnreclaim|PageTables|HugePages_Total|Hugetlb|Mapped|Dirty|Writeback:|KernelStack|VmallocUsed|Unevictable|Mlocked' /proc/meminfo
df -h /dev/shm /tmp 2>/dev/null | tail -2
