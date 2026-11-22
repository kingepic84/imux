#!/bin/bash
# window-size smallest|largest|latest|manual with two attached clients of different sizes,
# default-size for sessions nobody looks at, and new-session -x -y.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qws.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
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

width() {
  $BIN list-panes -t "$1" -F '#{pane_width}' 2>/dev/null | head -1
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

start() {
  printf '%s\n' "$1" >"$IMUX_CONFIG"
  $BIN kill-server >/dev/null 2>&1
  sleep 0.5
  $BIN new-session -d -s ws >/dev/null 2>&1
}

clients() {
  (sleep 1; eval "$1"; sleep 25) | timeout 60 script -q "$TMPDIR/a.out" sh -c "stty rows 30 cols 100; $BIN attach -t ws" >/dev/null 2>&1 &
  (sleep 1.5; eval "$2"; sleep 25) | timeout 60 script -q "$TMPDIR/b.out" sh -c "stty rows 20 cols 60; $BIN attach -t ws" >/dev/null 2>&1 &
  until_true '[ "$($BIN list-clients 2>/dev/null | wc -l | tr -d " ")" -ge 2 ]' 40
}

stop() {
  $BIN kill-server >/dev/null 2>&1
  wait 2>/dev/null
}

echo "-- smallest (the default)"
start 'set -g @none 1'
clients ':' ':'
assert "the window follows the smaller client" 'until_true "[ \"\$(width ws:)\" = 60 ]" 40'
stop

echo "-- largest"
start 'set -g window-size largest'
clients ':' ':'
assert "the window follows the larger client" 'until_true "[ \"\$(width ws:)\" = 100 ]" 40'
stop

echo "-- latest"
start 'set -g window-size latest'
clients 'sleep 6; printf x' 'sleep 1; printf y'
assert "the client that typed last sets the size (the small one)" 'until_true "[ \"\$(width ws:)\" = 60 ]" 40'
assert "and the other one when it types after" 'until_true "[ \"\$(width ws:)\" = 100 ]" 60'
stop

echo "-- manual and default-size"
start 'set -g window-size manual
set -g default-size 90x30'
clients ':' ':'
assert "manual ignores the clients" 'until_true "[ \"\$(width ws:)\" = 90 ]" 40'
$BIN set -g window-size smallest >/dev/null 2>&1
assert "changing the option applies at once" 'until_true "[ \"\$(width ws:)\" = 60 ]" 40'
stop

echo "-- sessions nobody looks at"
start 'set -g default-size 110x30'
$BIN new-session -d -s two >/dev/null 2>&1
$BIN new-session -d -s three -x 120 -y 40 >/dev/null 2>&1
$BIN new-session -d -s four -x 150 >/dev/null 2>&1
assert "default-size sizes a new detached session" '[ "$(width two:)" = 110 ]'
assert "-x -y size it explicitly" '[ "$(width three:)" = 120 ] && [ "$($BIN list-panes -t three: -F "#{pane_height}" | head -1)" -ge 38 ]'
assert "-x alone keeps the default height" '[ "$(width four:)" = 150 ]'
$BIN new-session -d -s five -x 5 >"$TMPDIR/bad.out" 2>&1
assert "a size out of range is refused" 'grep -q "bad width" "$TMPDIR/bad.out" && ! $BIN has-session -t five 2>/dev/null'
$BIN new-session -d -s six -y abc >"$TMPDIR/bad2.out" 2>&1
assert "and so is one that is no number" 'grep -q "bad height" "$TMPDIR/bad2.out"'
stop

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
