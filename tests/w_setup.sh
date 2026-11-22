#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-wsetup.XXXXXX)
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
set worktree-command "sleep 31342"
set worktree-setup "echo option >> $TMPDIR/opt.txt"
EOF

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

stop_commands() {
  pkill -f "sleep 31342" 2>/dev/null
  sleep 0.3
  return 0
}

(sleep 120; $BIN kill-server >/dev/null 2>&1; stop_commands) &

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

script_client() {
  (sleep 1; sleep 40; printf '\002d'; sleep 1) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s w"
}
script_client >/dev/null 2>&1 &
CLIENT=$!
sleep 3

lines() { wc -l <"$1" 2>/dev/null | tr -d ' '; }

echo "-- the option runs before the command"
(cd "$W/proj" && $BIN worktree ABC-1 >"$TMPDIR/o1" 2>&1)
sleep 3
assert "the setup of the option ran in the new window" '[ "$(lines "$TMPDIR/opt.txt")" = 1 ]'
assert "and the command started after it" 'pgrep -f "sleep 31342" >/dev/null'
stop_commands

echo "-- --no-setup runs the command alone"
(cd "$W/proj" && $BIN worktree --no-setup ABC-2 >"$TMPDIR/o2" 2>&1)
sleep 3
assert "no setup ran" '[ "$(lines "$TMPDIR/opt.txt")" = 1 ]'
assert "the command started" 'pgrep -f "sleep 31342" >/dev/null'
stop_commands

echo "-- the command waits for a slow setup"
(cd "$W/proj" && $BIN worktree --setup "sleep 2; touch $TMPDIR/slow.marker" ABC-3 >"$TMPDIR/o3" 2>&1)
sleep 1
assert "while the setup runs the marker is not there" '[ ! -e "$TMPDIR/slow.marker" ]'
assert "and neither is the command" '! pgrep -f "sleep 31342" >/dev/null'
sleep 4
assert "afterwards the setup has finished" '[ -e "$TMPDIR/slow.marker" ]'
assert "and the command runs" 'pgrep -f "sleep 31342" >/dev/null'
stop_commands

echo "-- a failing setup leaves the command unstarted"
(cd "$W/proj" && $BIN worktree --setup false ABC-4 >"$TMPDIR/o4" 2>&1)
sleep 3
assert "the command did not start" '! pgrep -f "sleep 31342" >/dev/null'

echo "-- an existing worktree skips the option's setup unless --setup is given"
(cd "$W/proj" && $BIN worktree ABC-3 >"$TMPDIR/o5" 2>&1)
check "it was reused" "reused" "$TMPDIR/o5"
sleep 2
assert "the option's setup did not run again" '[ "$(lines "$TMPDIR/opt.txt")" = 1 ]'
stop_commands
(cd "$W/proj" && $BIN worktree --setup "echo forced >> $TMPDIR/opt.txt" ABC-3 >"$TMPDIR/o6" 2>&1)
sleep 3
assert "an explicit --setup runs on a reused worktree" '[ "$(lines "$TMPDIR/opt.txt")" = 2 ]'
stop_commands

echo "-- variables are quoted, text with control characters is refused"
(cd "$W/proj" && $BIN worktree --setup "echo #{name} #{branch} >> $TMPDIR/vars.txt" ABC-5 >"$TMPDIR/o7" 2>&1)
sleep 3
check "the variables are replaced" "ABC-5 ABC-5" "$TMPDIR/vars.txt"
(cd "$W/proj" && $BIN worktree --setup "$(printf 'echo a\necho b')" ABC-6 >"$TMPDIR/o8" 2>&1)
check "a setup with a newline is refused with a reason" "control characters" "$TMPDIR/o8"
(cd "$W/proj" && $BIN worktree --setup "echo #{nope}" ABC-7 >"$TMPDIR/o9" 2>&1)
check "an unknown variable is refused with a reason" "not a variable" "$TMPDIR/o9"
stop_commands

kill "$CLIENT" 2>/dev/null
wait "$CLIENT" 2>/dev/null
$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
