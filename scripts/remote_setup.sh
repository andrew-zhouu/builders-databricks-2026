#!/usr/bin/env bash
# Point the repository at its GitHub remote and let WSL git use the Windows
# Git Credential Manager (repo-local settings only). Then report, without
# prompting, whether a push would be authorised.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
URL="${1:-https://github.com/andrew-zhouu/builders-databricks-2026.git}"
GCM="/mnt/c/Program Files/Git/mingw64/bin/git-credential-manager.exe"
git remote get-url origin >/dev/null 2>&1 && git remote set-url origin "$URL" || git remote add origin "$URL"
if [ -x "$GCM" ]; then
  git config credential.helper "$(printf '%q' "$GCM")"
  echo "credential helper: $(git config credential.helper)"
  echo "--- GitHub accounts known to the credential manager"
  "$GCM" github list 2>&1 | head -5
fi
echo "--- remote"; git remote -v
echo "--- push dry run (no prompt allowed)"
GCM_INTERACTIVE=Never GIT_TERMINAL_PROMPT=0 timeout 60 git push --dry-run origin main 2>&1 | tail -5
echo "(exit ${PIPESTATUS[0]})"
