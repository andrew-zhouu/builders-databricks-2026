#!/usr/bin/env bash
# Who is using the machine right now? (to avoid benchmarking on top of other agents)
uptime
free -m | head -2
ps -eo pid,pcpu,pmem,etimes,args --sort=-pcpu | head -${1:-12} | cut -c1-170
