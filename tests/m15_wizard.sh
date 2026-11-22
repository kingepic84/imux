#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-wizard.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"
REPOS="$TMPDIR/repos"

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 0
set repos-dir "$REPOS"
set worktree-base main
EOF

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) &

mkdir -p "$REPOS/beta-src" "$REPOS/plain" "$REPOS/.hidden"
for r in alpha beta; do
  git init -q --bare -b main "$TMPDIR/$r.git"
  git clone -q "$TMPDIR/$r.git" "$REPOS/$r" 2>/dev/null
  (cd "$REPOS/$r" && git checkout -q -b main && echo x >f && git add f && git commit -q -m init && git push -q origin main)
done
mkdir -p "$REPOS/.hidden/.git"

echo "-- list, cancel"
ARGS="new -s w" HOLD=1 client "$TMPDIR/a.out" '\002W' 'q' >/dev/null
check "the picker is titled" "repositories" "$TMPDIR/a.out"
check "lists a repository" "alpha" "$TMPDIR/a.out"
check "lists another" "beta" "$TMPDIR/a.out"
assert "plain directories are not listed" '! grep -aqF "plain" "$TMPDIR/a.out"'
assert "hidden ones are not listed" '! grep -aqF ".hidden" "$TMPDIR/a.out"'
assert "cancelling creates nothing" '[ ! -e "$REPOS/alpha-worktrees" ] && [ ! -e "$REPOS/beta-worktrees" ]'

echo "-- pick the second repository, name a worktree"
ARGS="attach -t w" HOLD=4 client "$TMPDIR/b.out" '\002W' 'j' '\r' 'ABC-9\r' >/dev/null
check "the name prompt shows" "worktree name" "$TMPDIR/b.out"
assert "the worktree is made in the picked repository" '[ -d "$REPOS/beta-worktrees/ABC-9" ]'
assert "and not in the other" '[ ! -e "$REPOS/alpha-worktrees" ]'
assert "on its new branch" '[ "$(git -C "$REPOS/beta-worktrees/ABC-9" symbolic-ref --short HEAD)" = ABC-9 ]'
$BIN ls >"$TMPDIR/ls" 2>&1
check "a window was added" "w: 2 windows" "$TMPDIR/ls"

echo "-- branch format with a slug"
ARGS="attach -t w" HOLD=4 client "$TMPDIR/d.out" '\002:' 'set worktree-branch-format feature/#{name}-#{slug}\r' '\002W' '\r' 'ABC-10 login-fix\r' >/dev/null
assert "the directory is the bare name" '[ -d "$REPOS/alpha-worktrees/ABC-10" ]'
assert "the branch has prefix and slug" '[ "$(git -C "$REPOS/alpha-worktrees/ABC-10" symbolic-ref --short HEAD)" = feature/ABC-10-login-fix ]'
ARGS="attach -t w" HOLD=4 client "$TMPDIR/f.out" '\002W' '\r' 'ABC-11\r' >/dev/null
assert "without a slug the dangling dash is dropped" '[ "$(git -C "$REPOS/alpha-worktrees/ABC-11" symbolic-ref --short HEAD)" = feature/ABC-11 ]'

echo "-- an empty repos-dir"
ARGS="attach -t w" HOLD=1 client "$TMPDIR/c.out" '\002:' 'set repos-dir /nonexistent-dir-x\r' '\002W' >/dev/null
check "a message says it cannot read it" "cannot read /nonexistent-dir-x" "$TMPDIR/c.out"

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
