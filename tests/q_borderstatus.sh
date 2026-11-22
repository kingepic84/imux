#!/bin/bash
# pane-border-status and pane-border-format: with top or bottom, every pane gives one row to a label
# row (its program gets one row less), the label is the format, and off gives the row back.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qz.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

record() {
  rm -f "$TMPDIR"/rec.*
  (sleep 2; sleep 1; printf '\002d'; sleep 0.5) | IMUX_RECORD_OUTPUT="$TMPDIR/rec" timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t sty" >/dev/null 2>&1
  cat "$TMPDIR"/rec.* >"$TMPDIR/all" 2>/dev/null
}

height() {
  $BIN display-message -p -t "$1" '#{pane_height}'
}

$BIN new-session -d -s sty -n one >/dev/null 2>&1
$BIN split-window -d -t sty:one >/dev/null 2>&1
sleep 1
P0=$($BIN list-panes -t sty:one -F '#{pane_id}' | head -1)
P1=$($BIN list-panes -t sty:one -F '#{pane_id}' | tail -1)
H0=$(height "$P0")
H1=$(height "$P1")

$BIN set -g pane-border-format 'LBL#{pane_index}X' >/dev/null 2>&1
$BIN set -g pane-border-status top >/dev/null 2>&1
sleep 1
assert "top: each pane loses one row" '[ "$(height "$P0")" = "$((H0 - 1))" ] && [ "$(height "$P1")" = "$((H1 - 1))" ]'
record
assert "top: the label is drawn for the first pane" 'LC_ALL=C grep -aq "LBL0X" "$TMPDIR/all"'
assert "top: and for the second" 'LC_ALL=C grep -aq "LBL1X" "$TMPDIR/all"'

T0=$(height "$P0")
$BIN set -g pane-border-status bottom >/dev/null 2>&1
sleep 1
assert "bottom: the same height as top" '[ "$(height "$P0")" = "$T0" ]'

$BIN split-window -d -t sty:one >/dev/null 2>&1
sleep 1
P2=$($BIN list-panes -t sty:one -F '#{pane_id}' | tail -1)
assert "a pane made while it is on has a height" '[ "$(height "$P2")" -ge 1 ]'

$BIN kill-pane -t "$P2" >/dev/null 2>&1
$BIN set -g pane-border-status off >/dev/null 2>&1
sleep 1
assert "off: the rows are given back" '[ "$(height "$P0")" = "$((T0 + 1))" ]'
$BIN set -g pane-border-status sideways >"$TMPDIR/o" 2>&1
assert "a bad value is refused" 'grep -aqi "pane-border-status" "$TMPDIR/o"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
