#!/bin/bash
# remain-on-exit (a pane whose program ended stays, pane-died, respawn-pane without -k) and
# focus-events (client-focus and pane-focus hooks, reports passed on to panes that asked).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-focus.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
T="$TMPDIR"
EV="$T/ev.out"
: >"$EV"

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

seen() {
  until_true "grep -aqxF -- \"$1\" $EV" 40
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cat >"$IMUX_CONFIG" <<EOF
set -g automatic-rename off
set -g remain-on-exit on
set-hook -g pane-died 'run-shell "echo pane-died:#{hook_exit_status} >> $EV"'
set-hook -g client-focus-out 'run-shell "echo client-focus-out >> $EV"'
set-hook -g client-focus-in 'run-shell "echo client-focus-in >> $EV"'
set-hook -g pane-focus-out 'run-shell "echo pane-focus-out:#{hook_pane} >> $EV"'
set-hook -g pane-focus-in 'run-shell "echo pane-focus-in:#{hook_pane} >> $EV"'
EOF

echo "-- remain-on-exit"
$BIN new-session -d -s f -n base >/dev/null 2>&1
$BIN new-window -d -t f -n dead "echo hi; exit 3" >/dev/null 2>&1
assert "pane-died fires with the exit status" 'seen pane-died:3'
$BIN list-panes -t f:dead -F '#{pane_dead} #{pane_dead_status}' >"$T/p1" 2>&1
assert "the pane is still there, marked dead with its status" 'grep -aqx "1 3" $T/p1'
$BIN capture-pane -p -t f:dead >"$T/c1" 2>&1
assert "it keeps what the program printed" 'grep -aq "^hi" $T/c1'
assert "and says it is dead" 'grep -aq "Pane is dead (status 3)" $T/c1'
$BIN send-keys -t f:dead x >"$T/o1" 2>&1
assert "typing into it is an error" 'grep -aq "dead" $T/o1'
$BIN respawn-pane -t f:dead "sleep 30" >"$T/o2" 2>&1
$BIN list-panes -t f:dead -F '#{pane_dead}' >"$T/p2" 2>&1
assert "respawn-pane needs no -k for a dead pane, and it lives again" 'grep -aqx "0" $T/p2'
$BIN kill-window -t f:dead >/dev/null 2>&1
$BIN set -g remain-on-exit failed >/dev/null 2>&1
$BIN new-window -d -t f -n ok "exit 0" >/dev/null 2>&1
until_true '! $BIN list-windows -t f -F "#{window_name}" 2>/dev/null | grep -aqx ok' 40
assert "with failed, a program that succeeded takes its pane with it" '! $BIN list-windows -t f -F "#{window_name}" | grep -aqx ok'
$BIN new-window -d -t f -n bad "exit 2" >/dev/null 2>&1
assert "with failed, one that failed stays" 'seen pane-died:2'
$BIN kill-window -t f:bad >/dev/null 2>&1
assert "kill-window removes a dead pane" '! $BIN list-windows -t f -F "#{window_name}" | grep -aqx bad'
$BIN set -g remain-on-exit off >/dev/null 2>&1
$BIN new-window -d -t f -n gone "exit 1" >/dev/null 2>&1
until_true '! $BIN list-windows -t f -F "#{window_name}" 2>/dev/null | grep -aqx gone' 40
assert "off is the old behavior" '! $BIN list-windows -t f -F "#{window_name}" | grep -aqx gone'

echo "-- focus-events"
$BIN new-window -t f -n ask "printf '\\033[?1004h'; stty raw -echo; head -c 6 | od -c >$T/got.out; sleep 30" >/dev/null 2>&1
sleep 1
(sleep 2; printf '\033[O'; sleep 1; printf '\033[I'; sleep 1; printf '\002d'; sleep 1) | timeout 30 script -q "$T/screen.out" sh -c "stty rows 24 cols 80; $BIN attach -t f" >/dev/null
assert "client-focus-out fires when the terminal loses the focus" 'seen client-focus-out'
assert "client-focus-in fires when it gets it back" 'seen client-focus-in'
assert "the reports reach a program that asked for them" 'grep -aq "033   \[   O" $T/got.out && grep -aq "033   \[   I" $T/got.out'
$BIN split-window -d -t f:ask >/dev/null 2>&1
ids=$($BIN list-panes -t f:ask -F '#{pane_id}')
first=$(echo "$ids" | sed -n 1p)
second=$(echo "$ids" | sed -n 2p)
$BIN select-pane -t "$second" >/dev/null 2>&1
assert "pane-focus-out names the pane that lost it" 'seen "pane-focus-out:$first"'
assert "pane-focus-in names the pane that got it" 'seen "pane-focus-in:$second"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
