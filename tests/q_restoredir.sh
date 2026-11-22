#!/bin/bash
# A pane restored into the home directory because its saved one is gone keeps the saved path in the
# next autosaves until the pane moves, says so once, and shows it in #{pane_restore_missing}.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qrd.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
export HOME="$TMPDIR/home"
mkdir -p "$HOME" "$TMPDIR/work"
fail=0
S="$IMUX_STATE_DIR"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

until_true() {
  local n=0
  while [ "$n" -lt "$2" ]; do
    eval "$1" && return 0
    sleep 0.25
    n=$((n + 1))
  done
  return 1
}

printf 'set autosave-interval 1\n' >"$IMUX_CONFIG"
(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN kill-server >/dev/null 2>&1
sleep 0.5
$BIN new-session -d -s r -c "$TMPDIR/work" >/dev/null 2>&1
until_true 'grep -aq "/work" "$S/autosave.imux" 2>/dev/null' 40
assert "the autosave names the working directory" 'grep -aq "/work" "$S/autosave.imux"'
SPID=$($BIN display-message -p '#{pid}' 2>/dev/null)
kill -TERM "$SPID" >/dev/null 2>&1
until_true '$BIN ls 2>&1 | grep -q "no server running"' 40
rm -rf "$TMPDIR/work"

(sleep 1; sleep 40) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s r" >/dev/null 2>&1 &
until_true '$BIN ls 2>/dev/null | grep -q "^r:"' 40
assert "the session came back" '$BIN ls | grep -q "^r:"'
assert "the pane is flagged" '[ "$($BIN list-panes -t r: -F "#{pane_restore_missing}" | head -1)" = 1 ]'
assert "the pane started in the home directory" 'until_true "[ \"\$($BIN display-message -p -t r: \"#{pane_current_path}\")\" = \"\$(cd \"$HOME\" && pwd -P)\" ]" 40'
assert "a message said the directory is gone" 'grep -aq "client [0-9]*: restore: .*/work is gone, the pane started in the home directory" "$TMPDIR/imux-$(id -u)/server.log"'

$BIN save >/dev/null 2>&1
assert "the next autosave still has the saved path" 'grep -aq "/work" "$S/autosave.imux"'
assert "and not the home directory in its place" '! grep -aq "^pane .*/home" "$S/autosave.imux"'

$BIN send-keys -t r: 'cd /tmp' Enter >/dev/null 2>&1
until_true '[ "$($BIN display-message -p -t r: "#{pane_current_path}")" != "$(cd "$HOME" && pwd -P)" ]' 40
$BIN save >/dev/null 2>&1
assert "once the pane moved the saved path is gone from the autosave" '! grep -aq "/work" "$S/autosave.imux"'
assert "and the flag is off" '[ "$($BIN list-panes -t r: -F "#{pane_restore_missing}" | head -1)" = 0 ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
