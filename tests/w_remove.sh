#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-wremove.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 0
set worktree-base main
set worktree-name-pattern "[A-Z]+-[0-9]+"
EOF

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) &

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
)
P="$W/proj"
WT="$W/proj-worktrees"

script_client() {
  (sleep 1; sleep 90; printf '\002d'; sleep 1) | timeout 120 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s w"
}
script_client >/dev/null 2>&1 &
CLIENT=$!
sleep 3

windows() { $BIN ls 2>&1 | grep -o '[0-9]* window' | head -1; }

echo "-- a clean worktree is removed, its window closes, its branch stays"
(cd "$P" && $BIN worktree ABC-1 >"$TMPDIR/o1" 2>&1)
sleep 1
assert "the window is open" '[ "$(windows)" = "2 window" ]'
(cd "$P" && $BIN worktree-remove ABC-1 >"$TMPDIR/r1" 2>&1)
sleep 1
check "it says what was removed" "removed worktree" "$TMPDIR/r1"
check "and that the branch is kept" "branch ABC-1 kept" "$TMPDIR/r1"
assert "the directory is gone" '[ ! -e "$WT/ABC-1" ]'
assert "git no longer lists it" '! git -C "$P" worktree list | grep -q ABC-1'
assert "the branch still exists" '[ -n "$(git -C "$P" branch --list ABC-1)" ]'
assert "the window is closed" '[ "$(windows)" = "1 window" ]'

echo "-- changes are refused, --force removes them"
(cd "$P" && $BIN worktree ABC-2 >"$TMPDIR/o2" 2>&1)
echo scratch >"$WT/ABC-2/new.txt"
(cd "$P" && $BIN worktree-remove ABC-2 >"$TMPDIR/r2" 2>&1)
check "a worktree with an untracked file is refused" "--force" "$TMPDIR/r2"
assert "and still there" '[ -d "$WT/ABC-2" ]'
(cd "$P" && $BIN worktree-remove --force ABC-2 >"$TMPDIR/r3" 2>&1)
check "--force removes it" "removed worktree" "$TMPDIR/r3"
assert "the directory is gone" '[ ! -e "$WT/ABC-2" ]'

echo "-- commits that no remote has are refused"
(cd "$P" && $BIN worktree ABC-3 >"$TMPDIR/o3" 2>&1)
(cd "$WT/ABC-3" && echo two >b.txt && git add b.txt && git commit -q -m two)
(cd "$P" && $BIN worktree-remove ABC-3 >"$TMPDIR/r4" 2>&1)
check "an unpushed commit is refused" "no remote" "$TMPDIR/r4"
assert "and the worktree is still there" '[ -d "$WT/ABC-3" ]'
(cd "$WT/ABC-3" && git push -q origin ABC-3 2>/dev/null)
(cd "$P" && $BIN worktree-remove ABC-3 >"$TMPDIR/r5" 2>&1)
check "once pushed it goes" "removed worktree" "$TMPDIR/r5"

echo "-- without a name, from inside the worktree"
(cd "$P" && $BIN worktree ABC-4 >"$TMPDIR/o4" 2>&1)
(cd "$WT/ABC-4" && $BIN worktree-remove >"$TMPDIR/r6" 2>&1)
check "the worktree the directory is in is removed" "removed worktree" "$TMPDIR/r6"
assert "the directory is gone" '[ ! -e "$WT/ABC-4" ]'

echo "-- what is never removed"
(cd "$P" && $BIN worktree-remove --force >"$TMPDIR/r7" 2>&1)
check "the main checkout" "main checkout" "$TMPDIR/r7"
assert "is still there" '[ -f "$P/a.txt" ]'
(cd "$P" && $BIN worktree-remove --force ABC-99 >"$TMPDIR/r8" 2>&1)
check "a name that is not a worktree" "not a worktree" "$TMPDIR/r8"
(cd "$P" && $BIN worktree ABC-5 >"$TMPDIR/o5" 2>&1)
git -C "$P" worktree lock "$WT/ABC-5"
(cd "$P" && $BIN worktree-remove --force ABC-5 >"$TMPDIR/r9" 2>&1)
check "a locked worktree, even with --force" "locked" "$TMPDIR/r9"
assert "is still there" '[ -d "$WT/ABC-5" ]'
(cd "$P" && $BIN worktree-remove ../../etc >"$TMPDIR/r10" 2>&1)
check "a name that is a path" "name" "$TMPDIR/r10"
(cd "$P" && $BIN worktree-remove a b c >"$TMPDIR/r11" 2>&1)
check "too many words is a usage error" "usage" "$TMPDIR/r11"

kill "$CLIENT" 2>/dev/null
wait "$CLIENT" 2>/dev/null
$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
