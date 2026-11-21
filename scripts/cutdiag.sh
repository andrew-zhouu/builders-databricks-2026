#!/usr/bin/env bash
# Cut the diagnostic image from `dev`, try it in Docker on the remote machine
# with the scorer's flags, and publish it under a fresh branch name (never a
# force-push). Prints the commit to submit.
# Usage: cutdiag.sh BRANCH_NAME        (e.g. diag3; the working tree must be clean)
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
name="$1"
bash scripts/mkdiag.sh dev | tail -1
bash scripts/remote.sh push > /dev/null
bash scripts/remote.sh export diag /home/ubuntu/img/diag > /dev/null
bash scripts/remote.sh run 'bash scripts/remote/image.sh diag /home/ubuntu/img/diag' > out/cutdiag.log 2>&1 || true
sed -n '1,2p' out/cutdiag.log
sed -n '/== diag: summary/,/model name/p' out/cutdiag.log | cut -c1-160
grep -q 'exit code: 1 ' out/cutdiag.log || { echo "the diagnostic image did not behave as intended"; exit 1; }
GCM_INTERACTIVE=Never GIT_TERMINAL_PROMPT=0 git push -q origin dev "diag:$name"
echo "diagnostic commit: $(git rev-parse diag) (branch $name)"
