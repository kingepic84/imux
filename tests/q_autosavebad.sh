#!/bin/bash
# A server that starts on an autosave which does not read back sets it aside, so the first save does
# not push the good previous copy out; the previous copy is what a restore uses.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qab.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
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
(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN kill-server >/dev/null 2>&1
sleep 0.5
$BIN new-session -d -s first >/dev/null 2>&1
$BIN new-window -t first: -n a >/dev/null 2>&1
until_true '[ -s "$S/autosave.imux" ]' 40
$BIN new-window -t first: -n b >/dev/null 2>&1
until_true '[ -s "$S/autosave.prev" ]' 40
assert "a good autosave and previous copy exist" '[ -s "$S/autosave.imux" ] && [ -s "$S/autosave.prev" ]'
SPID=$($BIN display-message -p '#{pid}' 2>/dev/null)
kill -TERM "$SPID" >/dev/null 2>&1
until_true '$BIN ls 2>&1 | grep -q "no server running"' 40
assert "the autosave stays after SIGTERM" '[ -s "$S/autosave.imux" ]'

cp "$S/autosave.prev" "$TMPDIR/prev.before"
printf 'imux-snapshot 1\nsav' >"$S/autosave.imux"

$BIN new-session -d -s second >/dev/null 2>&1
until_true 'ls "$S"/autosave.bad.* >/dev/null 2>&1' 40
assert "the damaged file was kept aside" 'ls "$S"/autosave.bad.* >/dev/null 2>&1'
assert "with its bytes" 'grep -aq "^sav$\|imux-snapshot 1" "$S"/autosave.bad.*'
assert "the good previous copy is untouched" 'cmp -s "$S/autosave.prev" "$TMPDIR/prev.before"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
