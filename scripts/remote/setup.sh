#!/usr/bin/env bash
# Runs ON the remote machine, once: system packages, the repository, the
# project-local toolchain, the official materials and the generator.
set -euo pipefail
REPO=https://github.com/andrew-zhouu/builders-databricks-2026.git
DIR="$HOME/db"
export DEBIAN_FRONTEND=noninteractive
if ! command -v docker >/dev/null || ! command -v unzip >/dev/null; then
  sudo apt-get update -qq
  sudo apt-get install -y -qq docker.io unzip linux-tools-common "linux-tools-$(uname -r)" >/dev/null 2>&1 \
    || sudo apt-get install -y -qq docker.io unzip linux-tools-aws >/dev/null
fi
sudo systemctl enable --now docker >/dev/null 2>&1 || true
sudo usermod -aG docker "$USER" || true
sudo chmod 666 /var/run/docker.sock
# Profiling: allow perf for the user, show kernel symbols.
sudo sysctl -q -w kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
docker --version; perf --version 2>&1 | head -1

if [ ! -d "$DIR/.git" ]; then
  mkdir -p "$DIR"
  # The directory may already hold files pushed with rsync: add the history around them.
  cd "$DIR"; git init -q -b dev; git remote add origin "$REPO"
fi
cd "$DIR"
git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' --tags
git checkout -q -f -B dev origin/dev
git branch -f main origin/main >/dev/null
git log --oneline -1; git tag -l

bash scripts/setup_toolchain.sh 2>&1 | grep -E '^(rustc|cargo|uv|polars)|installed' | head -8
bash scripts/fetch_official.sh 2>&1 | tail -7
bash scripts/build_generator.sh dev:1:1000000:250000 2>&1 | tail -4
bash scripts/build.sh 2>&1 | tail -1
bash scripts/check.sh 2>&1 | tail -3
echo SETUP-DONE
