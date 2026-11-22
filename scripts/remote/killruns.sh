#!/usr/bin/env bash
# Runs ON the remote machine: end the lead's own timing runs that hang (the
# program under test and the scripts driving it), and show what is left.
pkill -f 'scripts/abx.sh' ; pkill -f 'remote/phase.sh' ; pkill -f 'bin/pstat'
pkill -f 'release/sessions' ; pkill -f '/db/bin/'
sleep 1
pgrep -af 'sessions|abx|flock' | cut -c1-120 | head
uptime
