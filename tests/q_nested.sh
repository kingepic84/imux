#!/bin/bash
# A client attached from inside a pane of its own server is refused unless IMUX_ALLOW_NESTED=1;
# commands that only talk to the server still work there.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qn.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ALLOW_NESTED
fail=0

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

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g @none 1\n' >"$IMUX_CONFIG"
$BIN kill-server >/dev/null 2>&1
sleep 0.5
CMD="$BIN attach -t nest >$TMPDIR/o1 2>&1; echo rc=\$? >>$TMPDIR/o1"
CMD="$CMD; $BIN >$TMPDIR/o2 2>&1; echo rc=\$? >>$TMPDIR/o2"
CMD="$CMD; $BIN new -s other >$TMPDIR/o3 2>&1; echo rc=\$? >>$TMPDIR/o3"
CMD="$CMD; $BIN ls >$TMPDIR/o4 2>&1; echo rc=\$? >>$TMPDIR/o4"
CMD="$CMD; IMUX_ALLOW_NESTED=1 timeout 2 $BIN attach -t nest >$TMPDIR/o5 2>&1; echo rc=\$? >>$TMPDIR/o5"
CMD="$CMD; IMUX_SOCKET_PATH=/elsewhere/sock timeout 2 $BIN attach -t nest >$TMPDIR/o7 2>&1; echo rc=\$? >>$TMPDIR/o7"
CMD="$CMD; $BIN doctor >$TMPDIR/o6 2>&1; sleep 20"
$BIN new-session -d -s nest "$CMD" >/dev/null 2>&1
until_true '[ -s "$TMPDIR/o6" ]' 80

assert "attach is refused in a pane" 'grep -q "inside an imux pane" "$TMPDIR/o1" && grep -q "rc=1" "$TMPDIR/o1"'
assert "so is a bare imux" 'grep -q "inside an imux pane" "$TMPDIR/o2"'
assert "and imux new -s" 'grep -q "inside an imux pane" "$TMPDIR/o3"'
assert "the refusal names the way out" 'grep -q "IMUX_ALLOW_NESTED=1" "$TMPDIR/o1"'
assert "a command that only talks to the server works" 'grep -q "^nest:" "$TMPDIR/o4" && grep -q "rc=0" "$TMPDIR/o4"'
assert "IMUX_ALLOW_NESTED=1 lets a client attach" '! grep -q "inside an imux pane" "$TMPDIR/o5"'
assert "variables inherited from another server do not count as nesting" '! grep -q "inside an imux pane" "$TMPDIR/o7"'
assert "no second session was made by the refused new" '! $BIN ls | grep -q "^other:"'
assert "doctor says it" 'grep -q "^nested attach: refused" "$TMPDIR/o6"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
