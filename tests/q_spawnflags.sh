#!/bin/bash
# `-e NAME=VALUE` on new-window and split-window, and `split-window -b` (the new pane goes before).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qe.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 90; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s sp -n one >/dev/null 2>&1

id=$($BIN new-window -d -t sp -P -F '#{pane_id}' -e FOO=bar -e BAZ="a b" 'echo got-$FOO-$BAZ; sleep 30' 2>&1)
sleep 1
$BIN capture-pane -p -t "$id" >"$TMPDIR/cap1" 2>&1
assert "new-window -e gives the program its variables" 'grep -aq "got-bar-a b" "$TMPDIR/cap1"'

id=$($BIN split-window -d -t sp:0 -P -F '#{pane_id}' -e ZED=zz 'echo split-$ZED; sleep 30' 2>&1)
sleep 1
$BIN capture-pane -p -t "$id" >"$TMPDIR/cap2" 2>&1
assert "split-window -e too" 'grep -aq "split-zz" "$TMPDIR/cap2"'

$BIN new-window -d -t sp -e IMUX_PANE_ID=%99 >"$TMPDIR/e1" 2>&1
assert "an IMUX_ name is refused" 'grep -aq "IMUX_PANE_ID is set by imux" "$TMPDIR/e1"'
$BIN new-window -d -t sp -e NOEQUALS >"$TMPDIR/e2" 2>&1
assert "a pair without = is refused" 'grep -aq "NAME=VALUE" "$TMPDIR/e2"'
$BIN split-window -d -t sp:0 -e "bad-name=1" >"$TMPDIR/e3" 2>&1
assert "a bad name is refused" 'grep -aq "bad variable name" "$TMPDIR/e3"'
before=$($BIN list-windows -t sp | wc -l | tr -d ' ')
assert "nothing was made by the refused ones" '[ "$before" = 2 ]'

$BIN kill-pane -a -t "$($BIN list-panes -t sp:0 -F '#{pane_id}' | head -1)" >/dev/null 2>&1
$BIN select-pane -t sp:0.0 >/dev/null 2>&1
newid=$($BIN split-window -b -h -l 20 -t sp:0.0 -P -F '#{pane_id}' 2>&1)
first=$($BIN list-panes -t sp:0 -F '#{pane_id}' | head -1)
assert "split-window -b puts the new pane first" '[ "$first" = "$newid" ]'
width=$($BIN list-panes -t sp:0 -F '#{pane_id} #{pane_width}' | awk -v id="$newid" '$1 == id { print $2 }')
assert "-l sizes the new pane, not the old one" '[ "$width" = 20 ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
