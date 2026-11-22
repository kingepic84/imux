#!/bin/bash
# P10.3, spawning from a pane: the new pane's id comes back through -P -F, the focus stays,
# -c and -e reach the new pane, a split that would leave a pane under pane-min-width or
# pane-min-height is refused for a program in a pane (limit_reached, naming the options) and not
# for a split you type yourself, and IMUX_ names are refused by -e.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qsm.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
fail=0
seq=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 300; $BIN kill-server >/dev/null 2>&1; kill $$ >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\nset -g default-size 80x24\n' >"$IMUX_CONFIG"
HOLDER="$(cd "$(dirname "$0")" && pwd)/fixtures/pane_holder.sh"

inpane() {
  local pane=$1
  shift
  seq=$((seq + 1))
  rm -f "$TMPDIR/rc$seq"
  (trap '' PIPE; printf '%s\n' "{ $*; } >$TMPDIR/o$seq 2>&1; echo \$? >$TMPDIR/rc$seq" >"$TMPDIR/fifo$pane") >/dev/null 2>&1 &
  local i
  for i in $(seq 1 50); do
    [ -f "$TMPDIR/rc$seq" ] && break
    sleep 0.2
  done
  O="$TMPDIR/o$seq"
  RC=$(cat "$TMPDIR/rc$seq" 2>/dev/null)
  [ -f "$O" ] || : >"$O"
}

hold() {
  mkfifo "$TMPDIR/fifo$1"
  $BIN send-keys -t "%$1" -l "sh $HOLDER $TMPDIR/fifo$1" >/dev/null 2>&1
  $BIN send-keys -t "%$1" Enter >/dev/null 2>&1
  local i
  for i in $(seq 1 50); do
    [ -f "$TMPDIR/ready$1" ] && break
    (trap '' PIPE; printf '%s\n' "echo >$TMPDIR/ready$1" >"$TMPDIR/fifo$1") >/dev/null 2>&1 &
    sleep 0.2
  done
}

active() { $BIN list-panes -F '#{pane_id} #{pane_active}' | awk '$2 == 1 { print $1 }'; }
width() { $BIN display-message -p -t "$1" '#{pane_width}'; }

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s sm >/dev/null 2>&1
sleep 1
hold 0

echo "-- a spawn from a pane"
inpane 0 "$BIN split-window -h -P -F '#{pane_id}'"
assert "the id of the new pane comes back" '[ "$RC" = 0 ] && grep -aqx "%1" "$O"'
assert "the focus did not move" '[ "$(active)" = "%0" ]'
assert "the maker is recorded" '[ "$($BIN display-message -p -t %1 "#{pane_spawned_by}")" = "%0" ]'
inpane 0 "$BIN split-window -v -c /tmp -P -F '#{pane_id}'"
assert "a second spawn gets the next id" '[ "$RC" = 0 ] && grep -aqx "%2" "$O"'
sleep 1
assert "-c is where the new pane starts" '$BIN display-message -p -t %2 "#{pane_current_path}" | grep -aq "tmp$"'
assert "the focus still did not move" '[ "$(active)" = "%0" ]'

echo "-- the minimum size"
assert "the side by side panes are about half of 80 columns" '[ "$(width %1)" -ge 30 ] && [ "$(width %1)" -le 45 ]'
hold 1
inpane 1 "$BIN split-window -h -P -F '#{pane_id}'"
assert "a split that leaves under 20 columns is refused" '[ "$RC" != 0 ] && grep -aq "pane-min-width" "$O"'
assert "and says it is a limit, not a syntax problem" '! grep -aq "usage" "$O"'
assert "no pane was made" '[ "$($BIN list-panes -F "#{pane_id}" | wc -l | tr -d " ")" = 3 ]'
assert "the layout is as it was" '[ "$(width %1)" -ge 30 ]'
$BIN split-window -h -t %1 -P -F '#{pane_id}' >"$TMPDIR/user.out" 2>&1
assert "a split you type yourself has no such limit" 'grep -aqx "%[0-9]*" "$TMPDIR/user.out"'
$BIN kill-pane -t "$(head -1 "$TMPDIR/user.out")" >/dev/null 2>&1
sleep 0.5
$BIN set -g pane-min-width 10 >/dev/null 2>&1
inpane 1 "$BIN split-window -h -P -F '#{pane_id}'"
assert "with a lower pane-min-width the same split is allowed" '[ "$RC" = 0 ] && grep -aqx "%[0-9]*" "$O"'
$BIN set -g pane-min-width 20 >/dev/null 2>&1
$BIN set -g pane-min-height 30 >/dev/null 2>&1
inpane 0 "$BIN split-window -v -P -F '#{pane_id}'"
assert "pane-min-height refuses a split that leaves too few rows" '[ "$RC" != 0 ] && grep -aq "pane-min-height" "$O"'
$BIN set -g pane-min-height 5 >/dev/null 2>&1
assert "both options are options" '[ "$($BIN show-options -gv pane-min-width)" = 20 ] && [ "$($BIN show-options -gv pane-min-height)" = 5 ]'
assert "and are bounded" '! $BIN set -g pane-min-width 0 >/dev/null 2>&1'

echo "-- -e from a pane"
inpane 0 "$BIN new-window -d -e FOO=bar -P -F '#{pane_id}' \"sh -c 'echo ZZ\\\$FOO; sleep 30'\""
NEWP=$(head -1 "$O")
assert "a window with -e starts" '[ "$RC" = 0 ] && [ -n "$NEWP" ]'
sleep 1
assert "the variable is in its environment" '$BIN capture-pane -p -t "$NEWP" | grep -aq "^ZZbar"'
assert "the window did not become the current one" '[ "$($BIN display-message -p "#{window_index}")" = 0 ]'
inpane 0 "$BIN new-window -d -e IMUX_PANE_ID=9 -P -F '#{pane_id}'"
assert "-e never takes an IMUX_ name" '[ "$RC" != 0 ] && grep -aq "IMUX_" "$O"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
