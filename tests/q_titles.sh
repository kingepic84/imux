#!/bin/bash
# set-titles / set-titles-string: the terminal title a client is sent. Off by default; with it on a
# title goes out at attach and again when the text changes, never twice in a row, and what a pane
# put in its title cannot carry a control character into it.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qt.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0
ESC=$(printf '\033')

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s s -n one >/dev/null 2>&1

echo "-- off by default"
$BIN show-options set-titles >"$TMPDIR/o1" 2>&1
assert "set-titles is off" 'grep -aqx "set-titles off" "$TMPDIR/o1"'
(sleep 2; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/off.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1
assert "no title is sent" '! grep -aq "${ESC}\]2;" "$TMPDIR/off.out"'

echo "-- on"
$BIN set -g set-titles on >/dev/null 2>&1
$BIN set -g set-titles-string 'TT:#S:#I:#W:#{window_index}' >/dev/null 2>&1
(sleep 2; $BIN rename-window -t s:one two >/dev/null 2>&1; sleep 2; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/on.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1
assert "a title goes out at attach" 'grep -aqF "${ESC}]2;TT:s:0:one:0${ESC}\\" "$TMPDIR/on.out"'
assert "and a new one when the window is renamed" 'grep -aqF "${ESC}]2;TT:s:0:two:0${ESC}\\" "$TMPDIR/on.out"'
assert "the same title is not sent twice" '[ "$(grep -aoF "${ESC}]2;TT:s:0:one:0" "$TMPDIR/on.out" | wc -l)" -le 1 ]'

echo "-- a title from a pane"
$BIN set -g set-titles-string '[#T]' >/dev/null 2>&1
$BIN send-keys -t s:two 'printf "\033]2;pa\342\200\256ne\007"' Enter >/dev/null 2>&1
sleep 1
(sleep 2; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/pane.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1
assert "the pane's title is in it, without the bidirectional override" 'grep -aqF "${ESC}]2;[pane]${ESC}\\" "$TMPDIR/pane.out"'
assert "and the override itself is nowhere in the output" '! LC_ALL=C grep -aq "$(printf "\342\200\256")" "$TMPDIR/pane.out"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
