#!/bin/bash
# imux worktree list [repo]: path, branch, the window that shows it, notes; works without a server;
# a worktree whose branch already exists is checked out, not recreated.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qwl.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
fail=0

printf 'set autosave-interval 0\nset worktree-base main\nset worktree-name-pattern "[A-Z]+-[0-9]+"\n' >"$IMUX_CONFIG"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

W="$TMPDIR/work"
mkdir -p "$W"
git init -q --bare -b main "$TMPDIR/origin.git"
git clone -q "$TMPDIR/origin.git" "$W/proj" 2>/dev/null
(
  cd "$W/proj" || exit 1
  git checkout -q -b main
  echo one >a.txt
  git add a.txt
  git commit -q -m one
  git push -q origin main
  git branch ABC-9
)
P="$W/proj"

echo "-- no server"
$BIN kill-server >/dev/null 2>&1
(cd "$P" && $BIN worktree list >"$TMPDIR/l0" 2>&1)
assert "the main checkout is listed with its branch and a main note" 'grep -q "proj	main	-	main$" "$TMPDIR/l0"'
assert "from anywhere with a repository argument" '(cd "$TMPDIR" && $BIN worktree list "$P") | grep -q "proj	main"'
assert "outside a repository it says so" '(cd "$TMPDIR" && $BIN worktree list >"$TMPDIR/l1" 2>&1; grep -q "not in a git repository" "$TMPDIR/l1")'
assert "two words are a usage error" '! $BIN worktree list a b >"$TMPDIR/l2" 2>&1 && grep -q "usage: worktree list" "$TMPDIR/l2"'

echo "-- with a server and windows"
(sleep 1; sleep 40) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s w" >/dev/null 2>&1 &
sleep 3
(cd "$P" && $BIN worktree ABC-1 >"$TMPDIR/o1" 2>&1)
sleep 1
(cd "$P" && $BIN worktree list >"$TMPDIR/l3" 2>&1)
assert "the new worktree is listed on its branch" 'grep -q "ABC-1	ABC-1	w:[0-9]*	$" "$TMPDIR/l3"'
assert "with the window that shows it" 'grep -q "ABC-1	ABC-1	w:1" "$TMPDIR/l3"'
assert "and the main checkout shows no window" 'grep -q "proj	main	-	main$" "$TMPDIR/l3"'

echo "-- an existing branch is checked out"
(cd "$P" && $BIN worktree ABC-9 >"$TMPDIR/o9" 2>&1)
sleep 1
(cd "$P" && $BIN worktree list >"$TMPDIR/l4" 2>&1)
assert "the worktree is on the branch that existed" 'grep -q "ABC-9	ABC-9	w:" "$TMPDIR/l4"'
assert "and that branch still points where it did" '[ "$(git -C "$P" rev-parse ABC-9)" = "$(git -C "$P" rev-parse main)" ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
