#!/bin/bash
# `status off` and `status-position top|bottom`: where the bar is drawn, how many rows the panes
# get, and that a click on a top bar's tab selects the window.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qs.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"
ESC=$(printf '\033')

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

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

active() {
  $BIN list-windows -t s -F '#{window_name}=#{window_active}' 2>&1 | tr '\n' ' '
}

until_active() {
  local i
  for i in $(seq 1 40); do
    active | grep -aq -- "$1" && return 0
    sleep 0.25
  done
  return 1
}

echo "-- a bar at the bottom is the default"
$BIN new-session -d -s s -n one >/dev/null 2>&1
$BIN new-window -t s -d -n two >/dev/null 2>&1
$BIN set -g status-left BAR >/dev/null 2>&1
$BIN set -g status-right '' >/dev/null 2>&1
$BIN set -g window-status-format '#W' >/dev/null 2>&1
$BIN set -g window-status-current-format '#W' >/dev/null 2>&1
$BIN show-options status-position >"$TMPDIR/o1" 2>&1
assert "status-position shows its default" 'grep -aqx "status-position bottom" "$TMPDIR/o1"'
$BIN show-options status >"$TMPDIR/o2" 2>&1
assert "status shows its default" 'grep -aqx "status on" "$TMPDIR/o2"'

(sleep 14; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c1.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
assert "the panes get every row but the bar" 'until_height 29'
$BIN set -g status-position top >/dev/null 2>&1
assert "a top bar takes the same one row" 'until_height 29'
sleep 1
$BIN set -g status off >/dev/null 2>&1
assert "no bar gives the panes the whole screen" 'until_height 30'
$BIN set -g status on >/dev/null 2>&1
assert "on again takes the row back" 'until_height 29'
$BIN set -g status 3 >/dev/null 2>&1
assert "a row count is accepted as on" 'until_height 29'
$BIN set -g status on >/dev/null 2>&1
wait $CL
assert "a bar on top was drawn on the first row" 'grep -aqE "${ESC}\[1;1H(${ESC}\[[0-9;]*m)*BAR" "$TMPDIR/c1.out"'
assert "and one at the bottom on the last" 'grep -aqE "${ESC}\[30;1H(${ESC}\[[0-9;]*m)*BAR" "$TMPDIR/c1.out"'

echo "-- a client that starts with the bar off never sees it"
$BIN set -g status off >/dev/null 2>&1
(sleep 4; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c2.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
assert "its panes have all 30 rows" 'until_height 30'
wait $CL
assert "the bar text is nowhere in its output" '! grep -aq "BAR" "$TMPDIR/c2.out"'
$BIN set -g status on >/dev/null 2>&1

echo "-- a click on a tab of a top bar"
$BIN set -g status-position top >/dev/null 2>&1
$BIN select-window -t s:one >/dev/null 2>&1
assert "window one is current" 'active | grep -aq "one=1"'
(sleep 2; printf '\033[<0;9;1M\033[<0;9;1m'; sleep 2; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c3.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
assert "clicking the second tab on the first row selects it" 'until_active "two=1"'
wait $CL
$BIN select-window -t s:one >/dev/null 2>&1
(sleep 2; printf '\033[<0;9;30M\033[<0;9;30m'; sleep 2; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c4.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
sleep 3
assert "the same click on the last row (now a pane row) selects nothing" 'active | grep -aq "one=1"'
wait $CL

echo "-- the bar at the bottom still takes clicks there"
$BIN set -g status-position bottom >/dev/null 2>&1
$BIN select-window -t s:one >/dev/null 2>&1
(sleep 2; printf '\033[<0;9;30M\033[<0;9;30m'; sleep 2; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c5.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
assert "a click on the last row selects the second tab" 'until_active "two=1"'
wait $CL

echo "-- mistakes"
$BIN set -g status-position middle >"$TMPDIR/o3" 2>&1
assert "a bad position is refused" '[ $? -ne 0 ] && grep -aq "status-position" "$TMPDIR/o3"'
$BIN show-options status-position >"$TMPDIR/o4" 2>&1
assert "and the option keeps its value" 'grep -aqx "status-position bottom" "$TMPDIR/o4"'

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
