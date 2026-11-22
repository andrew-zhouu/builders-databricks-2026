#!/usr/bin/env bash
# Put the committed HEAD through everything a submission candidate needs, on
# the remote machine, as one detached job:
#   1. scripts/remote/fullcheck.sh on the pushed sources (fixtures, dev answer,
#      a large fuzz batch in all modes, six seeds, 5M/20M/20M/60M references);
#   2. scripts/remote/image.sh gate on a clean export of the commit (no-cache
#      Docker build as the scorer does it, no 512-bit instructions, fixtures and
#      dev/5M/20M/60M inside the container with the scorer's flags).
# Usage: cand.sh NAME [FUZZ_FIRST]     then: remote.sh log NAME 40
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
name="$1"; first="${2:-5000}"
[ -z "$(git status --porcelain)" ] || { echo "working tree not clean: commit first"; exit 1; }
rev=$(git rev-parse HEAD)
if git ls-tree -r HEAD | grep -q '^120000'; then echo "the commit holds a symbolic link: remove it first"; exit 1; fi
bash scripts/remote.sh push > /dev/null
bash scripts/remote.sh export "$rev" "/home/ubuntu/img/$name" > /dev/null
bash scripts/remote.sh tmux "$name" "echo candidate $rev; bash scripts/remote/fullcheck.sh 150 $first && bash scripts/remote/image.sh gate /home/ubuntu/img/$name m5 m20 m60 && echo CANDIDATE-OK $rev"
echo "candidate $rev: job $name started"
