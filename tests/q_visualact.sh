#!/bin/bash
# visual-activity: output in a window nobody views shows a message with the window number, once,
# when the flag goes up; with the option off the message is not shown.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qv.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s m -n cur >/dev/null 2>&1
$BIN setw -g monitor-activity on >/dev/null 2>&1
sleep 1

attach_and_print() {
  (sleep 4; printf '\002d'; sleep 0.5) | timeout 30 script -q "$1" sh -c "stty rows 30 cols 100; $BIN attach -t m" >/dev/null 2>&1 &
  CL=$!
  sleep 1.5
  $BIN new-window -d -t m -n "$2" >/dev/null 2>&1
  sleep 1
  $BIN send-keys -t "m:$2" 'echo noise' Enter
  wait $CL
}

attach_and_print "$TMPDIR/off.out" bg
assert "visual-activity off: no message" '! grep -aq "Activity in window" "$TMPDIR/off.out"'

$BIN set -g visual-activity on >/dev/null 2>&1
attach_and_print "$TMPDIR/on.out" bg2
assert "visual-activity on: the message names the window" 'grep -aq "Activity in window 2" "$TMPDIR/on.out"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
