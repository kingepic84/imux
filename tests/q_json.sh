#!/bin/bash
# P10.2, the machine-readable output: `--json` on agent list|get|wait|prompt (field names of the
# event stream, ids as strings), errors as {"error":{"code","message"}} on the standard error with
# exit 1 while the plain text of the same errors is unchanged, usage errors exit 2 in every mode,
# the pane defaulting to $IMUX_PANE_ID, the new `agent get` fields, and `imux api schema`.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qjson.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
fail=0
seq=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\nset -g agent-control on\nset -g restore-commands "claude"\n' >"$IMUX_CONFIG"

# Run the words, keep stdout in $OUT, stderr in $ERR and the status in $RC.
run() {
  OUT="$TMPDIR/out"
  ERR="$TMPDIR/err"
  timeout 20 $BIN "$@" >"$OUT" 2>"$ERR" </dev/null
  RC=$?
}

HOLDER="$(cd "$(dirname "$0")" && pwd)/fixtures/pane_holder.sh"

inpane() {
  shift
  seq=$((seq + 1))
  rm -f "$TMPDIR/rc$seq"
  printf '%s\n' "{ $*; } >$TMPDIR/o$seq 2>&1; echo \$? >$TMPDIR/rc$seq" >"$TMPDIR/fifo"
  local i
  for i in $(seq 1 50); do
    [ -f "$TMPDIR/rc$seq" ] && break
    sleep 0.2
  done
  O="$TMPDIR/o$seq"
  RC=$(cat "$TMPDIR/rc$seq" 2>/dev/null)
  [ -f "$O" ] || : >"$O"
}

$BIN kill-server >/dev/null 2>&1

echo "-- no server"
run agent list --json
assert "agent list --json with no server is server_unavailable on stderr, exit 1" '[ $RC = 1 ] && [ ! -s "$OUT" ] && grep -aq "\"code\":\"server_unavailable\"" "$ERR"'
run agent list
assert "the plain error is unchanged text on stdout" '[ $RC = 1 ] && grep -aq "^imux: no server running" "$OUT"'

$BIN new-session -d -s js >/dev/null 2>&1
sleep 1
mkfifo "$TMPDIR/fifo"
$BIN send-keys -t %0 -l "sh $HOLDER $TMPDIR/fifo" >/dev/null 2>&1
$BIN send-keys -t %0 Enter >/dev/null 2>&1
sleep 0.5

echo "-- the schema"
run api schema
assert "api schema is one JSON line" '[ $RC = 0 ] && [ "$(wc -l <"$OUT" | tr -d " ")" = 1 ] && grep -aq "^{\"api\":" "$OUT"'
for code in not_found ambiguous_target invalid_argument permission_denied limit_reached timeout agent_blocked agent_not_running prompt_stalled invalid_regex not_ready server_unavailable protocol_mismatch; do
  assert "the schema lists the code $code" 'grep -aq "\"code\":\"$code\"" "$OUT"'
done
run api version
assert "api version is unchanged" 'grep -aqx "api=[0-9]*" "$OUT"'

echo "-- a record"
inpane %0 "$BIN pane report-agent --source t --agent fake --state idle --message 'say \"hi\"' -- claude --continue"
assert "the pane reported" '[ "$RC" = 0 ]'
run agent get 0 --json
assert "agent get --json is one object" '[ $RC = 0 ] && [ "$(wc -l <"$OUT" | tr -d " ")" = 1 ] && grep -aq "^{" "$OUT"'
for field in session session_name window window_name pane state source agent message state_since seen seqs resume pid cwd; do
  assert "agent get --json has $field" 'grep -aq "\"$field\":" "$OUT"'
done
assert "ids are strings of the event forms" 'grep -aqE "\"session\":\"[\$][0-9]+\"" "$OUT" && grep -aqE "\"window\":\"@[0-9]+\"" "$OUT" && grep -aq "\"pane\":\"%0\"" "$OUT"'
assert "the state and the agent" 'grep -aq "\"state\":\"idle\"" "$OUT" && grep -aq "\"agent\":\"fake\"" "$OUT"'
want='"message":"say \"hi\""'
assert "a quote in the message is escaped" 'grep -aqF -- "$want" "$OUT"'
assert "the resume command is an array of words" 'grep -aq "\"resume\":\[\"claude\",\"--continue\"\]" "$OUT"'
assert "the source has its sequence" 'grep -aq "\"seqs\":{\"t\":[0-9]*}" "$OUT"'
assert "nobody has looked yet" 'grep -aq "\"seen\":false" "$OUT"'
run agent get 0
assert "the plain text keeps its lines" '[ $RC = 0 ] && grep -aq "^pane: 0$" "$OUT" && grep -aq "^state: idle$" "$OUT" && grep -aq "^message: say" "$OUT"'
assert "and has the new ones" 'grep -aq "^seen: no$" "$OUT" && grep -aq "^seq t: [0-9]*$" "$OUT"'
run agent list --json
assert "agent list --json is an array of those objects" '[ $RC = 0 ] && grep -aq "^\[{" "$OUT" && grep -aq "\"pane\":\"%0\"" "$OUT" && grep -aq "}\]$" "$OUT"'
run agent list
assert "agent list plain is unchanged" '[ $RC = 0 ] && grep -aq "^0	js:" "$OUT"'

echo "-- errors"
run agent get 99 --json
assert "a missing pane is not_found on stderr, exit 1, nothing on stdout" '[ $RC = 1 ] && [ ! -s "$OUT" ] && grep -aq "\"code\":\"not_found\"" "$ERR" && grep -aq "\"message\":\"no pane 99\"" "$ERR"'
run agent get 99
assert "the plain text has no code" '[ $RC = 1 ] && grep -aqx "imux: no pane 99" "$OUT"'
$BIN split-window -d >/dev/null 2>&1
sleep 0.5
run agent get 1 --json
assert "a pane with no record is agent_not_running and says why" '[ $RC = 1 ] && grep -aq "\"code\":\"agent_not_running\"" "$ERR" && grep -aq "no program has reported" "$ERR"'
run agent get 1
assert "plain: the same words, no code" '[ $RC = 1 ] && grep -aq "^imux: pane 1 has no agent record" "$OUT" && ! grep -aq "agent_not_running" "$OUT"'
run agent wait 0 --until working --timeout 1 --json
assert "a wait that runs out is timeout" '[ $RC = 1 ] && grep -aq "\"code\":\"timeout\"" "$ERR"'
run agent wait 0 --until working --timeout 1
assert "plain: timed out, exit 1" '[ $RC = 1 ] && grep -aq "timed out waiting for the agent" "$OUT"'
run agent wait 99 --json
assert "wait on a missing pane is not_found" '[ $RC = 1 ] && grep -aq "\"code\":\"not_found\"" "$ERR"'

echo "-- wait and prompt"
run agent wait 0 --until idle --json
assert "wait --json answers at once when the state is there" '[ $RC = 0 ] && grep -aqx "{\"pane\":\"%0\",\"state\":\"idle\"}" "$OUT"'
run agent prompt --json 0 hello there
assert "prompt --json says sent" '[ $RC = 0 ] && grep -aqE "^\{\"pane\":\"%0\",\"sent\":true(,\"seq\":[0-9]+)?\}$" "$OUT"'
run agent prompt 0 hello --json
assert "after the text, --json is text like --wait" '[ $RC = 0 ] && grep -aqx "sent" "$OUT"'
run agent prompt --json 1 hello
assert "prompt into a pane with no agent is agent_not_running" '[ $RC = 1 ] && grep -aq "\"code\":\"agent_not_running\"" "$ERR"'
$BIN set -g agent-control off >/dev/null 2>&1
run agent prompt --json 0 hello
assert "prompt with agent-control off is permission_denied naming the option" '[ $RC = 1 ] && grep -aq "\"code\":\"permission_denied\"" "$ERR" && grep -aq "agent-control" "$ERR"'
$BIN set -g agent-control on >/dev/null 2>&1
inpane %0 "$BIN pane report-agent --source t --agent fake --state blocked --message ask"
run agent prompt --json 0 hello
assert "prompt into a blocked agent is agent_blocked" '[ $RC = 1 ] && grep -aq "\"code\":\"agent_blocked\"" "$ERR"'
inpane %0 "$BIN pane report-agent --source t --agent fake --state idle"
assert "a blocked state shows no kind and no progress" '! $BIN agent get 0 --json | grep -aq "\"kind\""'

echo "-- the pane defaults to the pane of the caller"
inpane %0 "$BIN agent get"
assert "agent get with no pane in a pane means that pane" '[ "$RC" = 0 ] && grep -aq "^pane: 0$" "$O"'
inpane %0 "$BIN agent get --json"
assert "and with --json too" '[ "$RC" = 0 ] && grep -aq "\"pane\":\"%0\"" "$O"'
inpane %0 "$BIN agent get --current"
assert "--current is the same" '[ "$RC" = 0 ] && grep -aq "^pane: 0$" "$O"'
inpane %0 "$BIN agent wait --until idle"
assert "agent wait too" '[ "$RC" = 0 ] && grep -aqx "idle" "$O"'
inpane %0 "$BIN agent get 0 --current"
assert "a pane and --current together are refused" '[ "$RC" = 2 ]'

echo "-- usage errors exit 2 in every mode"
run agent get
assert "no pane and no environment: usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux agent get" "$OUT"'
run agent get --json
assert "the same with --json (the usage is text on stdout, not a JSON error)" '[ $RC = 2 ] && grep -aq "usage: imux agent get" "$OUT" && ! grep -aq "{\"error\":{" "$ERR"'
run agent get --current
assert "--current outside a pane is a usage error" '[ $RC = 2 ] && grep -aq "IMUX_PANE_ID" "$OUT"'
run agent wait --json
assert "agent wait with no pane: usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux agent wait" "$OUT"'
run agent prompt --json 0
assert "agent prompt with no text: usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux agent prompt" "$OUT"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
