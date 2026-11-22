#!/bin/bash
# P10.6, names of agents: `imux agent name PANE NAME|--clear`, the name in `agent get|list`, the
# agent commands and every `-t` taking a name (`agent:NAME`), uniqueness, the charset, a name going
# with its record (release, another program, a new record), the event, a snapshot round trip, and
# the rules for a program in a pane.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qan.XXXXXX)
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

printf 'set -g status off\nset -g default-size 100x30\nset -g agent-control on\nset -g restore-commands "claude"\n' >"$IMUX_CONFIG"
HOLDER="$(cd "$(dirname "$0")" && pwd)/fixtures/pane_holder.sh"

run() {
  OUT="$TMPDIR/out"
  ERR="$TMPDIR/err"
  timeout 30 $BIN "$@" >"$OUT" 2>"$ERR" </dev/null
  RC=$?
}

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

report() {
  inpane "$1" "$BIN pane report-agent --source t --agent ${3:-fake} --state ${2:-idle}"
}

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s an >/dev/null 2>&1
sleep 1
$BIN split-window -d >/dev/null 2>&1
$BIN split-window -d >/dev/null 2>&1
sleep 1
hold 0
hold 1
report 0
report 1

echo "-- giving a name"
run agent name 0 alpha
assert "agent name answers ok" '[ $RC = 0 ] && grep -aqx "ok" "$OUT"'
run agent get 0
assert "agent get shows the name" 'grep -aqx "name: alpha" "$OUT"'
run agent get 0 --json
assert "agent get --json has name next to agent" 'grep -aq "\"agent\":\"fake\",\"name\":\"alpha\"" "$OUT"'
run agent list
assert "agent list shows the name for the pane" 'grep -aq "^0	an:.*alpha" "$OUT" || grep -aq "alpha" "$OUT"'
run agent name 0 alpha --json
assert "the same name on the same pane is fine, --json says it" '[ $RC = 0 ] && grep -aqE "^\{\"pane\":\"%0\",\"name\":\"alpha\"(,\"seq\":[0-9]+)?\}$" "$OUT"'
run agent name 1 alpha --json
assert "a name in use is invalid_argument and says whose" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR" && grep -aq "pane 0" "$ERR"'
run agent name 1 beta
assert "another name for another pane" '[ $RC = 0 ]'
for bad in Bad 1a a:b a.b "a b" -x _x %1; do
  run agent name 1 -- "$bad"
  assert "the name '$bad' is refused" '[ $RC != 0 ]'
done
run agent name 1 "$(head -c 33 /dev/zero | tr '\0' a)" --json
assert "a name of 33 characters is refused" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR"'
run agent name 2 gamma --json
assert "a pane with no record cannot be named" '[ $RC = 1 ] && grep -aq "\"code\":\"agent_not_running\"" "$ERR"'
run agent name 99 gamma --json
assert "a missing pane is not_found" '[ $RC = 1 ] && grep -aq "\"code\":\"not_found\"" "$ERR"'
run agent name 1
assert "no name and no --clear is a usage error, exit 2" '[ $RC = 2 ]'

echo "-- the agent commands take the name"
run agent get beta
assert "agent get by name" '[ $RC = 0 ] && grep -aqx "pane: 1" "$OUT" && grep -aqx "name: beta" "$OUT"'
run agent get --json beta
assert "flags may come before the name" '[ $RC = 0 ] && grep -aq "\"pane\":\"%1\"" "$OUT"'
run agent wait beta --until idle --timeout 3
assert "agent wait by name" '[ $RC = 0 ] && grep -aqx "idle" "$OUT"'
run agent read alpha --lines 3
assert "agent read by name reads that pane" '[ $RC = 0 ]'
run agent read beta --lines 20
assert "and the right one" 'grep -aq "fifo1" "$OUT"'
run agent prompt beta hello there
assert "agent prompt by name" '[ $RC = 0 ] && grep -aqx "sent" "$OUT"'
run agent get nosuch --json
assert "a name that matches nothing is not_found" '[ $RC = 1 ] && grep -aq "\"code\":\"not_found\"" "$ERR" && grep -aq "nosuch" "$ERR"'
run agent get nosuch
assert "plain: the same text, no code, exit 1" '[ $RC = 1 ] && grep -aq "^imux: can.t find agent: nosuch" "$OUT"'
run agent get 2
assert "a number still means a pane" '[ $RC = 1 ] && grep -aq "pane 2 has no agent record" "$OUT"'

echo "-- -t agent:NAME"
run capture-pane -p -t agent:beta
assert "capture-pane -t agent:NAME reads that pane" '[ $RC = 0 ] && grep -aq "fifo1" "$OUT" && ! grep -aq "fifo0" "$OUT"'
run display-message -p -t agent:alpha '#{pane_id} #{agent_name}'
assert "display-message sees the pane and #{agent_name}" '[ $RC = 0 ] && grep -aqx "%0 alpha" "$OUT"'
run send-keys -t agent:beta -l ZZTOP-MARK
sleep 0.5
run capture-pane -p -t %1
assert "send-keys -t agent:NAME types into that pane" 'grep -aq "ZZTOP-MARK" "$OUT"'
run capture-pane -p -t %0
assert "and not into the other" '! grep -aq "ZZTOP-MARK" "$OUT"'
run capture-pane -p -t agent:nosuch
assert "a missing name is an error, not another target" '[ $RC != 0 ] && grep -aq "can.t find agent: nosuch" "$OUT" "$ERR"'
$BIN new-session -d -s agent >/dev/null 2>&1
run display-message -p -t agent:alpha '#{pane_id}'
assert "a session called agent does not take the target" '[ $RC = 0 ] && grep -aqx "%0" "$OUT"'
$BIN kill-session -t agent >/dev/null 2>&1

echo "-- the name goes with the record"
run agent name 1 --clear
assert "--clear takes the name off" '[ $RC = 0 ]'
run agent get 1
assert "the record has none" '[ $RC = 0 ] && ! grep -aq "^name:" "$OUT"'
run agent get beta --json
assert "and the name matches nothing" '[ $RC = 1 ] && grep -aq "not_found" "$ERR"'
$BIN agent name 1 delta >/dev/null 2>&1
report 1 working
run agent get delta
assert "a new state of the same program keeps the name" '[ $RC = 0 ] && grep -aqx "state: working" "$OUT"'
report 1 idle other
run agent get 1
assert "another program taking the pane takes the name" '[ $RC = 0 ] && ! grep -aq "^name:" "$OUT"'
$BIN agent name 1 delta >/dev/null 2>&1
inpane 1 "$BIN pane release-agent --source t"
run agent get delta --json
assert "a released record takes the name with it" '[ $RC = 1 ] && grep -aq "not_found" "$ERR"'
report 1 idle
run agent get 1
assert "a new record starts without one" '[ $RC = 0 ] && ! grep -aq "^name:" "$OUT"'

echo "-- the event"
timeout 20 $BIN events --types agent-state-changed >"$TMPDIR/events" 2>&1 </dev/null &
EVPID=$!
sleep 1
$BIN agent name 1 epsilon >/dev/null 2>&1
for i in $(seq 1 25); do grep -aq "\"name\":\"epsilon\"" "$TMPDIR/events" && break; sleep 0.2; done
assert "agent-state-changed carries the name when it is set" 'grep -aq "\"event\":\"agent-state-changed\".*\"name\":\"epsilon\"" "$TMPDIR/events"'
kill "$EVPID" 2>/dev/null

echo "-- from a pane"
run agent name 1 --clear
inpane 0 "$BIN agent name 0 alpha"
assert "a program may name the agent in its own pane" '[ "$RC" = 0 ]'
inpane 0 "$BIN agent name 1 zed"
assert "not the agent in a pane it did not spawn" '[ "$RC" != 0 ] && grep -aq "not one this pane spawned" "$O"'
run agent get 1
assert "and nothing changed" '! grep -aq "^name:" "$OUT"'
inpane 0 "$BIN agent get alpha"
assert "naming and looking up is free to look at" '[ "$RC" = 0 ] && grep -aqx "name: alpha" "$O"'
$BIN set -g command-from-panes off >/dev/null 2>&1
inpane 0 "$BIN agent name 0 other"
assert "with command-from-panes off it only looks" '[ "$RC" != 0 ]'
$BIN set -g command-from-panes spawned >/dev/null 2>&1

echo "-- a snapshot keeps the name"
run agent name 1 keeper
KIDX=$($BIN list-panes -t an -F '#{pane_id}' | grep -an '^%1$' | cut -d: -f1)
$BIN save snap1 >/dev/null 2>&1
assert "the snapshot file has the names" 'grep -rlq " +name alpha" "$IMUX_STATE_DIR" && grep -rlq " +name keeper" "$IMUX_STATE_DIR"'
$BIN kill-server >/dev/null 2>&1
sleep 1
$BIN new-session -d -s other >/dev/null 2>&1
sleep 1
run restore snap1
assert "the snapshot restores" '[ $RC = 0 ]'
sleep 2
run list-panes -a -F '#{session_name} #{pane_id}'
RESTORED=$(grep -a '^an ' "$OUT" | cut -d' ' -f2 | tr -d '%')
P0=$(printf '%s\n' "$RESTORED" | sed -n 1p)
P1=$(printf '%s\n' "$RESTORED" | sed -n "${KIDX}p")
assert "two panes came back" '[ -n "$P0" ] && [ -n "$P1" ]'
run agent get alpha
assert "no record yet, so the name matches nothing" '[ $RC = 1 ]'
$BIN pane run "$P0" "$BIN pane report-agent --source t --agent fake --state done" >/dev/null 2>&1
sleep 1.5
run agent get alpha --json
assert "the first record of the restored pane gets its name back" '[ $RC = 0 ] && grep -aq "\"pane\":\"%$P0\"" "$OUT"'
run agent get keeper --json
assert "the other one still waits for its program" '[ $RC = 1 ]'
$BIN pane run "$P0" "$BIN pane report-agent --source t --agent fake --state done" >/dev/null 2>&1
$BIN pane run "$P1" "$BIN pane report-agent --source t --agent fake --state done" >/dev/null 2>&1
sleep 1.5
run agent get keeper --json
assert "and gets it too" '[ $RC = 0 ] && grep -aq "\"pane\":\"%$P1\"" "$OUT"'
if [ "$fail" != 0 ]; then
  echo "--- diagnostics"
  $BIN agent list
  $BIN list-panes -a -F '#{session_name} #{pane_id} #{agent_name}'
  $BIN capture-pane -p -t "%$P1" | tail -5
fi

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
