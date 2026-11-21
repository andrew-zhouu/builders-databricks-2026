#!/usr/bin/env bash
# Run things on the remote development machine. Connection details live in
# .local/remote.env (not committed):  REMOTE_HOST, REMOTE_USER, REMOTE_KEY, REMOTE_DIR
#
#   remote.sh run 'command ...'        run a shell command in REMOTE_DIR
#   remote.sh script FILE [ARGS]       copy a local script over and run it with bash
#   remote.sh push                     rsync the working tree's sources to REMOTE_DIR
#   remote.sh tmux NAME 'command'      start a detached tmux session (long jobs)
#   remote.sh log NAME [LINES]         tail of a tmux session's log
#   remote.sh get REMOTE_PATH LOCAL    copy a file back
. "$(dirname "${BASH_SOURCE[0]}")/../scripts/env.sh"
set -euo pipefail
. "$PROJ/.local/remote.env"
SSH=(ssh -i "$REMOTE_KEY" -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30 -o ConnectTimeout=15
     -o ControlMaster=auto -o ControlPath="/tmp/ss-remote-%r@%h" -o ControlPersist=600 "$REMOTE_USER@$REMOTE_HOST")
cmd="$1"; shift
case "$cmd" in
  run)
    "${SSH[@]}" "mkdir -p $REMOTE_DIR && cd $REMOTE_DIR && $*" ;;
  script)
    f="$1"; shift
    "${SSH[@]}" "mkdir -p $REMOTE_DIR && cd $REMOTE_DIR && bash -s -- $*" < "$f" ;;
  push)
    # By checksum and without preserving times: a file that changed gets the
    # time of the transfer, so cargo over there always sees it as newer than its
    # last build (with preserved times, a file edited before a remote build but
    # pushed after it would be taken for unchanged).
    rsync -rlpzc --delete -e "ssh -i $REMOTE_KEY -o ControlPath=/tmp/ss-remote-%r@%h" \
      --exclude '/.git/' --exclude '/data/' --exclude '/out/' --exclude '/expected/' --exclude '/official/' \
      --exclude '/.toolchain/' --exclude '/.venv/' --exclude '/bin/' --exclude '/.local/' --exclude 'target/' \
      --exclude '/bench/prof-disasm.txt' --exclude '/bench/results/' --exclude '__pycache__/' \
      "$PROJ/" "$REMOTE_USER@$REMOTE_HOST:$REMOTE_DIR/" ;;
  tmux)
    name="$1"; shift
    "${SSH[@]}" "cd $REMOTE_DIR && mkdir -p out/logs && tmux new-session -d -s $name \"($*) > out/logs/$name.log 2>&1; echo EXIT=\\\$? >> out/logs/$name.log\" && echo started $name" ;;
  log)
    name="$1"; lines="${2:-40}"
    "${SSH[@]}" "cd $REMOTE_DIR && tail -n $lines out/logs/$name.log; tmux has-session -t $name 2>/dev/null && echo '[still running]' || echo '[finished]'" ;;
  export)
    # remote.sh export REV DIR: a clean export of a committed revision (as the scorer clones it)
    rev="$1"; dir="$2"
    git -C "$PROJ" archive "$rev" | "${SSH[@]}" "rm -rf $dir && mkdir -p $dir && tar -x -C $dir && echo exported $rev to $dir" ;;
  get)
    scp -q -i "$REMOTE_KEY" -o ControlPath="/tmp/ss-remote-%r@%h" "$REMOTE_USER@$REMOTE_HOST:$1" "$2" ;;
  *) echo "unknown command $cmd"; exit 2 ;;
esac
