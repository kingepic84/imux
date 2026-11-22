#!/bin/bash
# prefix2: a second key that starts a command like the prefix, `set prefix2 None` takes it away, and
# `send-prefix -2` types it into the pane. Also `BTab`, tmux's name of Shift-Tab, as a bound key.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qp2.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

keys() {
  (sleep 2; printf "$1"; sleep 1.5; printf '\002d'; sleep 0.5) \
    | timeout 30 script -q "$TMPDIR/k.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1
}

windows() { $BIN list-windows -t s 2>/dev/null | wc -l | tr -d ' '; }

$BIN new-session -d -s s -n one >/dev/null 2>&1
assert "show-options has prefix2 off by default" '$BIN show-options prefix2 2>&1 | grep -aqx "prefix2 None"'

keys '\001c'
assert "without prefix2, C-a is not a prefix: no new window" '[ "$(windows)" = 1 ]'

$BIN set -g prefix2 C-a >/dev/null 2>&1
keys '\001c'
assert "with prefix2 C-a, C-a c makes a window" '[ "$(windows)" = 2 ]'

keys '\002c'
assert "the first prefix still works" '[ "$(windows)" = 3 ]'

$BIN bind-key x send-prefix -2 >/dev/null 2>&1
$BIN send-keys -t s:0 C-u 'cat -v' Enter >/dev/null 2>&1
$BIN select-window -t s:0 >/dev/null 2>&1
sleep 1
keys '\002x'
$BIN capture-pane -p -t s:0 >"$TMPDIR/pane.txt" 2>&1
assert "send-prefix -2 types the second prefix into the pane" 'grep -aqF "^A" "$TMPDIR/pane.txt"'

$BIN set -g prefix2 None >/dev/null 2>&1
keys '\001c'
assert "after prefix2 None, C-a starts nothing again" '[ "$(windows)" = 3 ]'

$BIN bind-key BTab new-window >/dev/null 2>&1
keys '\002\033[Z'
assert "BTab as a bound key matches Shift-Tab after the prefix" '[ "$(windows)" = 4 ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
