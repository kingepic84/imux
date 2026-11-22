#!/bin/bash
# Phase 7 (S5/S7): a repository that carries a command in its own config (core.fsmonitor, which
# git runs on status and checkout) must not make `worktree` or `worktree-remove` run it.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-sgit.XXXXXX)
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

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) &

W="$TMPDIR/work"
mkdir -p "$W"
git init -q --bare -b main "$TMPDIR/origin.git"
git clone -q "$TMPDIR/origin.git" "$W/proj" 2>/dev/null
P="$W/proj"
(
  cd "$P" || exit 1
  git checkout -q -b main
  echo one >a.txt
  git add a.txt
  git commit -q -m one
  git push -q origin main
)

MARK="$TMPDIR/fsmonitor-ran"
printf '#!/bin/sh\ntouch "%s"\nprintf "\\0"\n' "$MARK" >"$TMPDIR/hook.sh"
chmod +x "$TMPDIR/hook.sh"
git -C "$P" config core.fsmonitor "$TMPDIR/hook.sh"

echo "-- control: plain git runs the repository's command"
git -C "$P" status >/dev/null 2>&1
assert "plain git status ran core.fsmonitor" '[ -e "$MARK" ]'
rm -f "$MARK"

$BIN new-session -d -s w || { echo "FAIL cannot start a session"; exit 1; }

echo "-- imux does not"
(cd "$P" && $BIN worktree ABC-1 >"$TMPDIR/o1" 2>&1)
assert "the worktree was made" '[ -d "$W/proj-worktrees/ABC-1" ]'
assert "worktree did not run core.fsmonitor" '[ ! -e "$MARK" ]'
(cd "$P" && $BIN worktree-remove ABC-1 >"$TMPDIR/r1" 2>&1)
assert "the worktree was removed" '[ ! -e "$W/proj-worktrees/ABC-1" ]'
assert "worktree-remove did not run core.fsmonitor" '[ ! -e "$MARK" ]'

echo "-- hostile names never reach git as options"
for name in -x --upload-pack=x '..' 'a b' "$(printf 'a\nb')" 'A;touch${IFS}'"$TMPDIR/pwned"; do
  (cd "$P" && $BIN worktree "$name" >"$TMPDIR/oh" 2>&1)
  assert "refused: $(printf '%q' "$name")" 'grep -aq "bad worktree name\|does not match\|usage\|unknown" "$TMPDIR/oh"'
done
assert "nothing was run from a name" '[ ! -e "$TMPDIR/pwned" ]'

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
