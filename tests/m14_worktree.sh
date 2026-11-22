#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-worktree.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 0
set worktree-base develop
set worktree-name-pattern "[A-Z]+-[0-9]+"
set worktree-command "sleep 31338"
EOF

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

cleanup_processes() {
  pkill -f "sleep 31338" 2>/dev/null
  return 0
}

(sleep 120; $BIN kill-server >/dev/null 2>&1; cleanup_processes) &

W="$TMPDIR/work"
mkdir -p "$W"
git init -q --bare -b develop "$TMPDIR/origin.git"
git clone -q "$TMPDIR/origin.git" "$W/proj" 2>/dev/null
(
  cd "$W/proj" || exit 1
  git checkout -q -b develop
  echo one >a.txt
  git add a.txt
  git commit -q -m one
  git push -q origin develop
  git checkout -q -b main
  git push -q origin main
  git checkout -q -b feature
  echo mine >>a.txt
  echo scratch >untracked.txt
)
git clone -q "$TMPDIR/origin.git" "$TMPDIR/other" 2>/dev/null
(
  cd "$TMPDIR/other" || exit 1
  git checkout -q develop
  echo two >b.txt
  git add b.txt
  git commit -q -m two
  git push -q origin develop
)
NEWSHA=$(git -C "$TMPDIR/other" rev-parse HEAD)
MAINSHA=$(git -C "$TMPDIR/origin.git" rev-parse main)

script_client() {
  (sleep 1; sleep 30; printf '\002d'; sleep 1) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s w"
}
script_client >/dev/null 2>&1 &
CLIENT=$!
sleep 3

echo "-- a worktree off the fetched base"
(cd "$W/proj" && $BIN worktree ABC-1 >"$TMPDIR/o1" 2>&1)
check "reports the base" "worktree ABC-1 from origin/develop" "$TMPDIR/o1"
assert "directory beside the repo" '[ -d "$W/proj-worktrees/ABC-1" ]'
assert "branch is cut from the new origin/develop, not the checkout" '[ "$(git -C "$W/proj-worktrees/ABC-1" rev-parse HEAD)" = "$NEWSHA" ]'
assert "worktree is on the new branch" '[ "$(git -C "$W/proj-worktrees/ABC-1" symbolic-ref --short HEAD)" = ABC-1 ]'
assert "the main checkout is still on its branch" '[ "$(git -C "$W/proj" symbolic-ref --short HEAD)" = feature ]'
assert "the main checkout keeps its changes" 'grep -q mine "$W/proj/a.txt" && [ -f "$W/proj/untracked.txt" ]'
$BIN ls >"$TMPDIR/ls1" 2>&1
check "a window was added" "w: 2 windows" "$TMPDIR/ls1"
sleep 2
assert "the command runs in a pane" 'pgrep -f "sleep 31338" >/dev/null'

echo "-- again, detached, explicit base"
(cd "$W/proj" && $BIN worktree ABC-1 >"$TMPDIR/o2" 2>&1)
check "second call reuses" "reused" "$TMPDIR/o2"
(cd "$W/proj" && $BIN worktree --detach ABC-2 >"$TMPDIR/o3" 2>&1)
assert "detached head" '! git -C "$W/proj-worktrees/ABC-2" symbolic-ref -q HEAD >/dev/null'
assert "detached at origin/develop" '[ "$(git -C "$W/proj-worktrees/ABC-2" rev-parse HEAD)" = "$NEWSHA" ]'
(cd "$W/proj" && $BIN worktree ABC-3 main >"$TMPDIR/o4" 2>&1)
assert "explicit base" '[ "$(git -C "$W/proj-worktrees/ABC-3" rev-parse HEAD)" = "$MAINSHA" ]'
(cd "$W" && $BIN worktree proj ABC-6 >"$TMPDIR/o9" 2>&1)
assert "repo given as the first word" '[ -d "$W/proj-worktrees/ABC-6" ]'

echo "-- origin offline"
git -C "$W/proj" remote set-url origin "$TMPDIR/gone.git"
(cd "$W/proj" && $BIN worktree ABC-4 >"$TMPDIR/o5" 2>&1)
check "warns about the fetch" "warning: fetch failed" "$TMPDIR/o5"
assert "still created from the last known origin" '[ -d "$W/proj-worktrees/ABC-4" ]'
git -C "$W/proj" remote set-url origin "$TMPDIR/origin.git"

echo "-- failures"
(cd "$W/proj" && $BIN worktree abc >"$TMPDIR/e1" 2>&1)
check "a name outside the pattern is refused" "does not match worktree-name-pattern" "$TMPDIR/e1"
(cd "$W/proj" && $BIN worktree ABC-5 nope >"$TMPDIR/e2" 2>&1)
check "a missing base is named" "origin/nope" "$TMPDIR/e2"
assert "nothing left behind for it" '[ ! -e "$W/proj-worktrees/ABC-5" ]'
(cd "$TMPDIR" && $BIN worktree ABC-7 >"$TMPDIR/e3" 2>&1)
check "outside a repository" "not in a git repository" "$TMPDIR/e3"
$BIN worktree >"$TMPDIR/e4" 2>&1
check "no name is a usage error" "usage: worktree" "$TMPDIR/e4"

$BIN ls >"$TMPDIR/ls2" 2>&1
check "windows for the successful calls only" "w: 7 windows" "$TMPDIR/ls2"

wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1
cleanup_processes
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
