#!/bin/bash
# A reported resume command is typed again for every restored pane when the snapshot is restored,
# whether its window is in front or not and whether anyone looks at its session or not: the
# commands do not wait for a focus.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qrr.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
export SHELL=/bin/sh
FIX="$(cd "$(dirname "$0")" && pwd)/fixtures/report_resume.sh"
fail=0

printf 'set autosave-interval 0\nset restore-commands "sleep"\n' >"$IMUX_CONFIG"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s w >/dev/null 2>&1
$BIN new-window -d -t w >/dev/null 2>&1
sleep 2
panes=$($BIN list-panes -a -F '#{pane_id}' -t w 2>/dev/null)
first=$(printf '%s\n' "$panes" | sed -n 1p)
second=$(printf '%s\n' "$panes" | sed -n 2p)
assert "two panes in two windows, the first window in front" '[ -n "$first" ] && [ -n "$second" ] && [ "$first" != "$second" ]'
$BIN send-keys -t "$first" "$FIX 31341" Enter >/dev/null 2>&1
$BIN send-keys -t "$second" "$FIX 31342" Enter >/dev/null 2>&1
sleep 3
$BIN save resumeB >"$TMPDIR/save.txt" 2>&1
assert "both reports are in the snapshot" '[ "$(grep -c "+resume" "$IMUX_STATE_DIR/snapshots/resumeB.imux")" = 2 ]'

$BIN kill-server >/dev/null 2>&1
sleep 1
pkill -f "sleep 41" 2>/dev/null

echo "-- restore with a client on another session"
(sleep 12; printf '\002d'; sleep 1) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s other" >/dev/null 2>&1 &
sleep 3
$BIN restore resumeB >"$TMPDIR/rest.txt" 2>&1
sleep 4
assert "the pane of the window in front is resumed" 'pgrep -f "sleep 31341" >/dev/null'
assert "the pane of the window behind it is resumed too" 'pgrep -f "sleep 31342" >/dev/null'
assert "the restored session is not the one the client looks at" '! $BIN list-clients -F "#{client_session}" | grep -qx w'

$BIN kill-server >/dev/null 2>&1
sleep 1
pkill -f "sleep 31341" 2>/dev/null
pkill -f "sleep 31342" 2>/dev/null
pkill -f "sleep 41" 2>/dev/null
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
