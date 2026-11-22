#!/usr/bin/env bash
# Runs ON the remote machine: end the lead's own jobs that the change of scorer
# made pointless (the gate of the old candidate, the queued index A/B). Other
# sessions' jobs are left alone.
tmux kill-session -t w2 2>/dev/null && echo "killed tmux w2" || echo "no tmux w2"
pkill -f 'remote/wixab.sh' && echo "killed wixab" || echo "no wixab"
pkill -f 'remote/fullcheck.sh 150 12000' && echo "killed the w2 fullcheck" || true
pkill -f 'image.sh gate /home/ubuntu/img/w2' || true
sleep 1
tmux ls 2>/dev/null
pgrep -af 'flock /tmp/ss-heavy.lock' | cut -c1-160
tail -3 "$HOME/db/out/logs/w2.log" | cut -c1-120
