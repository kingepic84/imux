#!/bin/bash
# Phase 7 (S3/S7): what a hostile program in a pane can do to the server. An OSC string that never
# ends must not grow the server, text that reorders or injects must not reach the status data,
# and the server must go on answering.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-spanes.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
SOCKDIR="$TMPDIR/imux-$(id -u)"
LOG="$SOCKDIR/server.log"

printf 'set autosave-interval 0\n' >"$IMUX_CONFIG"

ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

(sleep 170; $BIN kill-server >/dev/null 2>&1) &

wait_for() {
  local tries=$1; shift
  for _ in $(seq "$tries"); do
    if "$@" >/dev/null 2>&1; then return 0; fi
    sleep 0.3
  done
  return 1
}

pane_has() { $BIN capture-pane -p -t %0 | grep -aqF -- "$1"; }
type_line() { $BIN send-keys -t %0 -l "$1" && $BIN send-keys -t %0 Enter; }

$BIN new-session -d -s w || { echo "FAIL cannot start a session"; exit 1; }
wait_for 30 pane_has '$' || wait_for 30 pane_has '%' || true
SERVER=$(lsof -t "$SOCKDIR/default" 2>/dev/null | head -1)
[ -n "$SERVER" ] && ok "found the server process ($SERVER)" || bad "no server process"

echo "-- the socket is private"
[ "$(stat -f %Lp "$SOCKDIR/default")" = 600 ] && ok "socket mode 600" || bad "socket mode"
[ "$(stat -f %Lp "$SOCKDIR")" = 700 ] && ok "socket directory mode 700" || bad "socket directory mode"
[ "$(stat -f %Lp "$LOG")" = 600 ] && ok "server log mode 600" || bad "server log mode $(stat -f %Lp "$LOG")"

echo "-- an OSC string that never ends does not grow the server"
RSS0=$(ps -o rss= -p "$SERVER" | tr -d ' ')
type_line "(printf '\\033]0;'; head -c 200000000 /dev/zero | tr '\\0' a); echo FLOODED"
if wait_for 200 pane_has 'FLOODED'; then
  ok "the pane carried on after 200 MB of OSC text"
else
  bad "the flood did not finish"
fi
RSS1=$(ps -o rss= -p "$SERVER" | tr -d ' ')
GROWTH=$(( (RSS1 - RSS0) / 1024 ))
if [ "$GROWTH" -lt 64 ]; then ok "server grew ${GROWTH} MB (limit 64)"; else bad "server grew ${GROWTH} MB"; fi
$BIN ls >/dev/null 2>&1 && ok "the server still answers" || bad "the server does not answer"
type_line "printf 'after-%s\\n' flood"
wait_for 30 pane_has 'after-flood' && ok "the pane shows text after the flood" || bad "no text after the flood"

echo "-- a string that ends is still run, a long title is cut"
type_line "printf '\\033]0;%s\\007' \"\$(head -c 100000 /dev/zero | tr '\\0' t)\"; echo TITLED"
wait_for 30 pane_has 'TITLED' && ok "a 100 KB title does not disturb the pane" || bad "pane stuck after a long title"

echo "-- bidirectional and control characters are refused in agent text"
type_line "$BIN pane report-agent --source t --agent x --state working --message \"\$(printf 'a\\342\\200\\256b')\" 2>&1 | sed 's/^/R1:/'"
wait_for 30 pane_has 'R1:imux' && ok "report with a right-to-left override answered" || bad "no answer"
$BIN capture-pane -p -t %0 | grep -a 'R1:' | grep -aqi 'bidirectional' && ok "refused for the bidirectional character" || bad "not refused"
type_line "$BIN pane report-agent --source t --agent x --state working --message fine 2>&1 | sed 's/^/R2:/'; echo R2DONE"
wait_for 30 pane_has 'R2DONE' && ok "a plain message passes" || bad "plain message not answered"
$BIN capture-pane -p -t %0 | grep -a 'R2:' | grep -aqi 'refused\|error\|bidirectional' && bad "plain message refused" || ok "plain message not refused"

echo "-- events escape bidirectional characters"
$BIN events --types window-renamed >"$TMPDIR/ev.txt" 2>&1 &
EVP=$!
sleep 1
$BIN rename-window -t w "$(printf 'x\342\200\256y')" >/dev/null 2>&1
sleep 1
kill "$EVP" 2>/dev/null
wait "$EVP" 2>/dev/null
grep -aq 'window-renamed' "$TMPDIR/ev.txt" && ok "the rename produced an event" || bad "no rename event"
if grep -aq "$(printf '\342\200\256')" "$TMPDIR/ev.txt"; then bad "raw right-to-left override in an event"; else ok "no bidirectional character in the event stream"; fi

echo "-- a pane that never reads its input cannot freeze the server"
answers() {
  "$BIN" ls >/dev/null 2>&1 &
  local p=$!
  for _ in $(seq 20); do
    kill -0 "$p" 2>/dev/null || { wait "$p"; return $?; }
    sleep 0.25
  done
  kill "$p" 2>/dev/null
  return 1
}
$BIN new-window -d -t w "stty -icanon -echo; i=0; while [ \$i -lt 4000 ]; do printf '\\033[c\\033[6n'; i=\$((i+1)); done; sleep 120" >/dev/null 2>&1
sleep 3
stuck=0
for _ in 1 2 3 4; do answers || stuck=1; sleep 1; done
[ "$stuck" = 0 ] && ok "the server answered while a pane filled its own input queue" || bad "the server stopped answering"
type_line "echo STILLHERE"
wait_for 30 pane_has 'STILLHERE' && ok "the first pane still takes input" || bad "the first pane is stuck"

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
