#!/bin/bash
# P10.8, `agent prompt --wait` hardening: a settled state answers only after the agent was seen
# working or blocked since the text (or was working when it came), an agent that shows neither in
# 5 seconds is prompt_stalled, blocked and error answer at once, another program taking the pane
# ends the wait, and one timeout covers typing and waiting and says delivery is unknown.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qpg.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
HOLDER="$(cd "$(dirname "$0")" && pwd)/fixtures/pane_holder.sh"
fail=0
seq=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 300; $BIN kill-server >/dev/null 2>&1; kill $$ >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\nset -g default-size 100x30\nset -g agent-control on\nset -g agent-stall-time 60\n' >"$IMUX_CONFIG"

inpane() {
  local pane=$1
  shift
  seq=$((seq + 1))
  rm -f "$TMPDIR/rc$seq"
  (trap '' PIPE; printf '%s\n' "{ $*; } >$TMPDIR/o$seq 2>&1; echo \$? >$TMPDIR/rc$seq" >"$TMPDIR/fifo$pane") >/dev/null 2>&1 &
  local i
  for i in $(seq 1 100); do
    [ -f "$TMPDIR/rc$seq" ] && break
    sleep 0.1
  done
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

report() {
  inpane "$1" "$BIN pane report-agent --source t --agent ${3:-fake} --state $2"
}

prompt_bg() {
  rm -f "$TMPDIR/pout" "$TMPDIR/perr" "$TMPDIR/prc"
  (timeout 60 $BIN agent prompt --wait --json --timeout "${2:-30}" "$1" hello >"$TMPDIR/pout" 2>"$TMPDIR/perr" </dev/null; echo $? >"$TMPDIR/prc") &
  sleep 1
}

presult() {
  local i
  for i in $(seq 1 $(( ${1:-15} * 10 ))); do
    [ -f "$TMPDIR/prc" ] && break
    sleep 0.1
  done
  PRC=$(cat "$TMPDIR/prc" 2>/dev/null)
}

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s pg >/dev/null 2>&1
sleep 1
for n in 1 2 3 4 5 6 7; do
  $BIN new-window -d -t pg >/dev/null 2>&1
done
sleep 1
PANES=$($BIN list-panes -a -F '#{pane_id}' | tr -d '%')
set -- $PANES
A=$1; B=$2; C=$3; D=$4; E=$5; F=$6; G=$7
for p in $A $B $C $D $E $F $G; do
  hold "$p"
  report "$p" idle
done

echo "-- a quick turn"
prompt_bg "$A"
report "$A" working
report "$A" idle
presult 10
assert "working then idle answers idle" '[ "$PRC" = 0 ] && grep -aq "\"state\":\"idle\"" "$TMPDIR/pout"'

echo "-- a settled state with no activity stalls"
$BIN set -g agent-stall-time 5 >/dev/null 2>&1
prompt_bg "$B"
report "$B" done
sleep 1
assert "an idle or done after the text does not answer" '[ ! -f "$TMPDIR/prc" ]'
presult 12
$BIN set -g agent-stall-time 60 >/dev/null 2>&1
assert "five seconds with no working or blocked is prompt_stalled" '[ "$PRC" = 1 ] && grep -aq "\"code\":\"prompt_stalled\"" "$TMPDIR/perr"'
assert "and says delivery is unknown" 'grep -aq "not delivered" "$TMPDIR/perr"'

echo "-- blocked answers at once"
prompt_bg "$C"
report "$C" blocked
presult 5
assert "blocked after the text answers blocked" '[ "$PRC" = 0 ] && grep -aq "\"state\":\"blocked\"" "$TMPDIR/pout"'

echo "-- the one timeout"
prompt_bg "$D" 2
presult 6
assert "a timeout under five seconds is timeout, not a stall" '[ "$PRC" = 1 ] && grep -aq "\"code\":\"timeout\"" "$TMPDIR/perr" && grep -aq "not delivered" "$TMPDIR/perr"'

echo "-- another program takes the pane"
prompt_bg "$E"
report "$E" working other
presult 5
assert "another agent taking the pane ends the wait" '[ "$PRC" = 1 ] && grep -aq "\"code\":\"agent_not_running\"" "$TMPDIR/perr" && grep -aq "another agent" "$TMPDIR/perr"'

echo "-- an agent that was working when the text came"
report "$F" working
prompt_bg "$F"
sleep 6
assert "no stall while it works" '[ ! -f "$TMPDIR/prc" ]'
report "$F" done
presult 5
assert "the end of the turn answers" '[ "$PRC" = 0 ] && grep -aq "\"state\":\"done\"" "$TMPDIR/pout"'

echo "-- error answers at once"
prompt_bg "$G"
report "$G" error
presult 5
assert "error after the text answers error" '[ "$PRC" = 0 ] && grep -aq "\"state\":\"error\"" "$TMPDIR/pout"'

if [ "$fail" != 0 ]; then
  echo "--- diagnostics"
  $BIN agent list
  command cat "$TMPDIR/pout" "$TMPDIR/perr" 2>/dev/null
fi

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
