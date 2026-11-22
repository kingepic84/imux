#!/bin/bash
# imux doctor names the panes that started in the home directory because their saved directory is
# gone, and says nothing about them once they have moved.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qdr.XXXXXX)
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
SPID=$($BIN display-message -p '#{pid}' 2>/dev/null)
kill -TERM "$SPID" >/dev/null 2>&1
until_true '$BIN ls 2>&1 | grep -q "no server running"' 40
assert "doctor says nothing about restore without a server" '! $BIN doctor 2>&1 | grep -q "^restore:"'
rm -rf "$TMPDIR/work"

(sleep 1; sleep 40) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s r" >/dev/null 2>&1 &
until_true '$BIN ls 2>/dev/null | grep -q "^r:"' 40
$BIN doctor >"$TMPDIR/d1" 2>&1
assert "doctor names the pane that lost its directory" 'grep -q "^restore: 1 pane(s) started in the home directory because their saved directory is gone: r:[0-9]*\.[0-9]*$" "$TMPDIR/d1"'

$BIN send-keys -t r: 'cd /tmp' Enter >/dev/null 2>&1
until_true '[ "$($BIN list-panes -t r: -F "#{pane_restore_missing}" | head -1)" = 0 ]' 40
$BIN save >/dev/null 2>&1
$BIN doctor >"$TMPDIR/d2" 2>&1
assert "once the pane moved the line is gone" '! grep -q "^restore:" "$TMPDIR/d2"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
