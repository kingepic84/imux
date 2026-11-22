#!/bin/bash
# P10.9, waiting for events without a race: `imux events --wait EVENT [--match key=value]...
# [--timeout MS]`, `--since SEQ` (replay of the kept events, events_lost when some are gone), the
# sequence number `api-seq` gives and that ends the `--json` result of a command that changes
# something.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qew.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 200; $BIN kill-server >/dev/null 2>&1; kill $$ >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\nset -g default-size 100x30\nset -g agent-control on\n' >"$IMUX_CONFIG"

run() {
  OUT="$TMPDIR/out"
  ERR="$TMPDIR/err"
  timeout 30 $BIN "$@" >"$OUT" 2>"$ERR" </dev/null
  RC=$?
}

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s ew -n first >/dev/null 2>&1
sleep 1

echo "-- the sequence number"
run api-seq
assert "api-seq prints a number" '[ $RC = 0 ] && grep -aqE "^[0-9]+$" "$OUT"'
S0=$(tr -d '\n' <"$OUT")
$BIN rename-window -t ew:0 one >/dev/null 2>&1
sleep 0.5
run api-seq
S1=$(tr -d '\n' <"$OUT")
assert "an event raises it" '[ "$S1" -gt "$S0" ]'

echo "-- --wait"
(timeout 20 $BIN events --wait window-renamed --match window_name=waited >"$TMPDIR/w1" 2>"$TMPDIR/w1err" </dev/null; echo $? >"$TMPDIR/w1rc") &
sleep 1
$BIN rename-window -t ew:0 other >/dev/null 2>&1
sleep 0.5
assert "an event that does not match is not the answer" '[ ! -f "$TMPDIR/w1rc" ]'
$BIN rename-window -t ew:0 waited >/dev/null 2>&1
for i in $(seq 1 50); do [ -f "$TMPDIR/w1rc" ] && break; sleep 0.2; done
assert "the matching event is printed and the exit status is 0" '[ "$(cat $TMPDIR/w1rc)" = 0 ] && grep -aq "\"event\":\"window-renamed\"" "$TMPDIR/w1" && grep -aq "\"window_name\":\"waited\"" "$TMPDIR/w1"'
assert "and only that line is printed" '[ "$(wc -l <"$TMPDIR/w1" | tr -d " ")" = 1 ]'
run events --wait pane-exited --timeout 700
assert "no event in time is a timeout, exit 1" '[ $RC = 1 ] && grep -aq "in time" "$ERR" "$OUT"'
run events --wait window-renamed --match nokey
assert "a match is key=value" '[ $RC = 2 ]'
run events --match window_name=x
assert "--match needs --wait" '[ $RC = 2 ]'
run events --wait window-renamed --types pane-exited
assert "--wait and --types do not mix" '[ $RC = 2 ]'
run events --wait nosuch-event --timeout 500
assert "an event name that is not one is an error" '[ $RC != 0 ]'

echo "-- --since"
run api-seq
N=$(tr -d '\n' <"$OUT")
$BIN rename-window -t ew:0 early >/dev/null 2>&1
sleep 0.3
$BIN rename-window -t ew:0 late >/dev/null 2>&1
sleep 0.5
run events --wait window-renamed --match window_name=early --since "$N" --timeout 3000
assert "an event that came before the wait started is found" '[ $RC = 0 ] && grep -aq "\"window_name\":\"early\"" "$OUT"'
run events --wait window-renamed --match window_name=late --since "$N" --timeout 3000
assert "so is a later one" '[ $RC = 0 ] && grep -aq "\"window_name\":\"late\"" "$OUT"'
run events --wait window-renamed --match window_name=early --timeout 700
assert "without --since the past is not seen" '[ $RC = 1 ]'
timeout 2 $BIN events --since "$N" --types window-renamed >"$TMPDIR/replay" 2>&1 </dev/null
assert "a replay starts with the hello line and has the kept events of the named type" 'head -1 "$TMPDIR/replay" | grep -aq "\"event\":\"subscribed\"" && [ "$(grep -ac "\"event\":\"window-renamed\"" "$TMPDIR/replay")" = 2 ] && ! grep -aq "events_lost" "$TMPDIR/replay"'
FIRST=$(grep -a "window-renamed" "$TMPDIR/replay" | head -1 | grep -ao "\"seq\":[0-9]*" | cut -d: -f2)
assert "the replay is in order and after the number" '[ "$FIRST" -gt "$N" ]'
run events --since 99999999 --types window-renamed
assert "a number from a longer life is events_lost, exit 1" '[ $RC = 1 ] && grep -aq "\"event\":\"events_lost\"" "$OUT"'
run events --since x
assert "a --since that is not a number is a usage error" '[ $RC = 2 ]'

echo "-- the sequence number of a change"
run pane run --json 0 true
assert "pane run --json ends with seq" '[ $RC = 0 ] && grep -aqE "^\{\"pane\":\"%0\",\"sent\":true,\"seq\":[0-9]+\}$" "$OUT"'
run pane run 0 true
assert "plain output has no seq" '[ $RC = 0 ] && grep -aqx sent "$OUT"'
run pane run --json 0 true
SEQ=$(grep -ao "\"seq\":[0-9]*" "$OUT" | cut -d: -f2)
$BIN rename-window -t ew:0 afterrun >/dev/null 2>&1
sleep 0.5
run events --wait window-renamed --match window_name=afterrun --since "$SEQ" --timeout 3000
assert "act, then wait from the noted number: the event is found" '[ $RC = 0 ]'
run api schema
assert "the schema names seq" 'grep -aq "last event before the change" "$OUT"'

if [ "$fail" != 0 ]; then
  echo "--- diagnostics"
  command cat "$TMPDIR/w1" "$TMPDIR/w1err" "$TMPDIR/replay" 2>/dev/null | head -20
fi

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
