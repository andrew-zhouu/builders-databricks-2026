#!/usr/bin/env bash
# Pre-publication survey: remote state, credential helpers, and anything in the
# history that should not go public.
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
cd "$PROJ"
URL="${1:-https://github.com/andrew-zhouu/builders-databricks-2026.git}"
echo "--- remote"; GIT_TERMINAL_PROMPT=0 git ls-remote "$URL" 2>&1 | head -5; echo "(exit ${PIPESTATUS[0]})"
echo "--- credential helpers"
git config --show-origin --get-all credential.helper 2>/dev/null || echo "none configured in WSL git"
ls "/mnt/c/Program Files/Git/mingw64/bin/git-credential-manager.exe" "/mnt/c/Program Files/Git/mingw64/libexec/git-core/git-credential-manager.exe" 2>/dev/null
echo "--- status"; git status --short | head -20; git branch -a; git log --oneline | head -25
echo "--- history scan (addresses, keys, tokens)"
for rev in $(git rev-list --all); do
  git grep -nIE '[A-Za-z0-9._%+-]+@gmail\.com|BEGIN [A-Z ]*PRIVATE KEY|ghp_[A-Za-z0-9]{20,}|AKIA[0-9A-Z]{16}|\.pem' "$rev" -- . 2>/dev/null
done | sed 's/^\([0-9a-f]\{7\}\)[0-9a-f]*/\1/' | sort -u | head -40
echo "--- size"; git count-objects -vH | grep -E 'size-pack|count'
echo "--- largest tracked files"; git ls-files -z | xargs -0 du -b 2>/dev/null | sort -rn | head -5
