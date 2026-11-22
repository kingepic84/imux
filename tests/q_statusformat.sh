#!/bin/bash
# status-format[N]: a row drawn from its own text. A format on row 1 adds a row the panes give up;
# a format on row 0 replaces the standard bar; clearing the formats brings the bar back.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qf.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0
ESC=$(printf '\033')

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

height() {
  $BIN display-message -p -t s:one '#{pane_height}' 2>&1
}

until_height() {
  local i
  for i in $(seq 1 40); do
    [ "$(height)" = "$1" ] && return 0
    sleep 0.25
  done
  return 1
}

$BIN new-session -d -s s -n one >/dev/null 2>&1
$BIN set -g status-left BAR >/dev/null 2>&1
$BIN set -g status-right '' >/dev/null 2>&1

(sleep 16; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c1.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
assert "one bar row to begin with" 'until_height 29'
$BIN set -g status-format[1] 'SECOND ROW' >/dev/null 2>&1
assert "a format on row 1 adds a row" 'until_height 28'
$BIN set -g status-format[0] 'FIRST ROW' >/dev/null 2>&1
sleep 1
assert "a format on row 0 keeps both rows" 'until_height 28'
$BIN set -gu status-format[1] >/dev/null 2>&1
assert "clearing row 1 gives the row back" 'until_height 29'
$BIN set -gu status-format[0] >/dev/null 2>&1
sleep 1
wait $CL
assert "the second row was drawn on the last screen row" 'grep -aqE "${ESC}\[30;1H(${ESC}\[[0-9;]*m)*SECOND ROW" "$TMPDIR/c1.out"'
assert "the first row text was drawn above it" 'grep -aqE "${ESC}\[29;1H(${ESC}\[[0-9;]*m)*FIRST ROW" "$TMPDIR/c1.out"'

$BIN set -g status-format[7] x >"$TMPDIR/o1" 2>&1
assert "an index past 4 is refused" '[ $? -ne 0 ] && grep -aq "status-format" "$TMPDIR/o1"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
