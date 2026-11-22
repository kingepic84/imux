#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-agents.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 0
set agent-glyph-working "WWW"
set agent-glyph-blocked "BBB"
set agent-glyph-done "DDD"
set status-right " G:#{agent_glyph} N:#{agent_count} "
set window-status-format " #I#{window_agent_glyph} "
set window-status-current-format " #I*#{window_agent_glyph} "
EOF

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 170; $BIN kill-server >/dev/null 2>&1) &

cat >"$TMPDIR/probe.sh" <<'EOF'
#!/bin/bash
printf '\033]7501;?\007'
IFS= read -rs -t 4 -d '\' reply </dev/tty
printf '%s' "$reply" | cat -v >"$1"
EOF
chmod +x "$TMPDIR/probe.sh"

echo "-- the pane answers the status query"
ARGS="new -s w" HOLD=3 client "$TMPDIR/a.out" "$TMPDIR/probe.sh $TMPDIR/reply.txt\r" >/dev/null
sleep 1
check "reply names the states" "states=idle,working,done,blocked,error" "$TMPDIR/reply.txt"
check "reply names the kinds" "kinds=permission,question,auth" "$TMPDIR/reply.txt"

$BIN kill-server >/dev/null 2>&1
sleep 1

cat >"$TMPDIR/agent.sh" <<'EOF'
#!/bin/bash
for step in "$@"; do
  case $step in
    wait:*) sleep "${step#wait:}" ;;
    *) printf '\033]7501;%s\033\\' "$step" ;;
  esac
done
sleep 30
EOF
chmod +x "$TMPDIR/agent.sh"
A="$TMPDIR/agent.sh"

echo "-- status glyphs follow the reports"
ARGS="new -s w" HOLD=6 client "$TMPDIR/b.out" \
  "$A state=working:app=fake\r" \
  '\002c' "$A state=blocked:app=fake:kind=permission wait:2 state=working:app=fake\r" \
  '\002c' "$A wait:2 state=done:app=fake\r" \
  '\002' '0' >/dev/null
check "a working pane shows the working glyph" "WWW" "$TMPDIR/b.out"
check "a blocked pane shows the blocked glyph" "BBB" "$TMPDIR/b.out"
check "a finished background pane shows the done glyph" "DDD" "$TMPDIR/b.out"
check "several panes in one state are counted" "N:2" "$TMPDIR/b.out"
assert "the window list carries a glyph per window" 'LC_ALL=C grep -aqE "\[30;([1-7][0-9]|[0-9])H[^W]{0,80}WWW" "$TMPDIR/b.out"'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- a finished pane stops counting once it has been looked at"
ARGS="new -s w2" HOLD=7 client "$TMPDIR/c.out" \
  '\002c' "$A wait:2 state=done:app=fake\r" \
  '\002' '0' 'wait' >/dev/null
check "the done glyph shows while the pane is out of sight" "DDD" "$TMPDIR/c.out"
ARGS="attach -t w2" HOLD=2 client "$TMPDIR/d.out" '\002' '1' >/dev/null
assert "after looking at it the glyph is gone from what follows" '! tail -c 400 "$TMPDIR/d.out" | grep -aqF "DDD"'

$BIN kill-server >/dev/null 2>&1
sleep 1

cat >"$TMPDIR/reader.sh" <<'EOF'
#!/bin/bash
printf '\033]7501;state=blocked:app=fake:kind=permission:msg=YXBwcm92ZS10aGluZw==\033\\'
IFS= read -r line </dev/tty
printf '%s' "$line" >"$1"
sleep 30
EOF
chmod +x "$TMPDIR/reader.sh"

echo "-- the agent panel lists panes and jumps to the chosen one"
ARGS="new -s w3" HOLD=1 client "$TMPDIR/e.out" '\002A' >/dev/null
check "with no agents a message says so" "no agents reporting" "$TMPDIR/e.out"
$BIN kill-server >/dev/null 2>&1
sleep 1
ARGS="new -s w4" HOLD=3 client "$TMPDIR/f.out" \
  '\002c' "$TMPDIR/reader.sh $TMPDIR/got.txt\r" \
  '\002c' "$A state=working:app=fake\r" \
  '\002c' '\002' 'A' \
  '\r' "hello-from-the-panel\r" >/dev/null
check "the panel has a title" "agents" "$TMPDIR/f.out"
check "it shows the message of a blocked agent" "approve-thing" "$TMPDIR/f.out"
check "it shows a working agent too" "fake: working" "$TMPDIR/f.out"
sleep 1
assert "Enter jumps to the first row, the blocked agent, and keys go there" '[ "$(cat "$TMPDIR/got.txt" 2>/dev/null)" = "hello-from-the-panel" ]'

$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- reports over the socket, from inside a pane"
cat >"$TMPDIR/conf2" <<EOF
set autosave-interval 0
set restore-commands "claude vim"
EOF
export IMUX_CONFIG="$TMPDIR/conf2"
cat >"$TMPDIR/reports.sh" <<EOF
#!/bin/bash
echo "env-\$IMUX_ENV-\$IMUX_PANE_ID" >"$TMPDIR/env.txt"
R="$BIN pane report-agent --source hook --agent fake"
\$R --state working --seq 5 --message hi >"$TMPDIR/r1.txt" 2>&1
\$R --state idle --seq 3 >"$TMPDIR/r2.txt" 2>&1
\$R --state blocked --seq 6 -- rm -rf x >"$TMPDIR/r3.txt" 2>&1
\$R --state blocked --seq 7 -- claude --resume abc >"$TMPDIR/r4.txt" 2>&1
$BIN pane report-agent --source other --agent fake --state done --seq 1 >"$TMPDIR/r5.txt" 2>&1
$BIN pane release-agent --source nobody >"$TMPDIR/r6.txt" 2>&1
sleep 40
EOF
chmod +x "$TMPDIR/reports.sh"
ARGS="new -s w5" HOLD=10 client "$TMPDIR/g.out" "$TMPDIR/reports.sh\r" >/dev/null &
CLIENT=$!
sleep 7
$BIN agent list >"$TMPDIR/l1.txt" 2>&1
$BIN agent get 0 >"$TMPDIR/get1.txt" 2>&1
check "panes get the environment" "env-1-0" "$TMPDIR/env.txt"
check "a report is accepted" "ok" "$TMPDIR/r1.txt"
check "a stale report is ignored" "stale" "$TMPDIR/r2.txt"
check "a resume command that is not allowed is refused" "resume command refused" "$TMPDIR/r3.txt"
check "an allowed one is taken" "ok" "$TMPDIR/r4.txt"
check "another source may write too" "ok" "$TMPDIR/r5.txt"
check "the list shows the pane and its state" "done" "$TMPDIR/l1.txt"
check "get shows the writer" "source: other" "$TMPDIR/get1.txt"
check "get shows the resume command" "resume: claude --resume abc" "$TMPDIR/get1.txt"
check "a source that did not write the record cannot release it" "nothing to release" "$TMPDIR/r6.txt"
$BIN pane report-agent 0 --source x --agent a --state idle >"$TMPDIR/spoof.txt" 2>&1
check "a report from outside the pane is refused" "is not where this command runs" "$TMPDIR/spoof.txt"
$BIN agent wait 0 --until working --timeout 1 >"$TMPDIR/w1.txt" 2>&1
check "waiting past the timeout fails" "timed out" "$TMPDIR/w1.txt"
$BIN agent wait 0 --until done >"$TMPDIR/w2.txt" 2>&1
check "waiting for a state the agent is already in answers at once" "done" "$TMPDIR/w2.txt"
$BIN agent get 99 >"$TMPDIR/nopane.txt" 2>&1
check "an unknown pane is an error" "no pane 99" "$TMPDIR/nopane.txt"
wait $CLIENT

echo "-- agent wait is answered when the state arrives"
ARGS="new -s w6" HOLD=8 client "$TMPDIR/h.out" \
  "$BIN pane report-agent --source hook --agent fake --state working --seq 1 >/dev/null 2>&1\r" \
  "(sleep 3; $BIN pane report-agent --source hook --agent fake --state done --seq 2) >/dev/null 2>&1 &\r" >/dev/null &
CLIENT=$!
sleep 4
start=$(date +%s)
$BIN agent wait 0 --until done --timeout 20 >"$TMPDIR/w3.txt" 2>&1
took=$(( $(date +%s) - start ))
check "the wait returns the state" "done" "$TMPDIR/w3.txt"
assert "and returns when it arrives, not at the timeout" '[ "$took" -lt 15 ]'
wait $CLIENT

$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- a reported resume command comes back after a restart"
cat >"$TMPDIR/conf3" <<EOF
set autosave-interval 0
set restore-commands "sleep"
EOF
export IMUX_CONFIG="$TMPDIR/conf3"
cat >"$TMPDIR/resumer.sh" <<EOF
#!/bin/bash
$BIN pane report-agent --source hook --agent fake --state idle -- sleep 31339 >"$TMPDIR/rr.txt" 2>&1
sleep 41
EOF
chmod +x "$TMPDIR/resumer.sh"
ARGS="new -s w7" HOLD=9 client "$TMPDIR/i.out" "$TMPDIR/resumer.sh\r" >/dev/null &
CLIENT=$!
sleep 5
$BIN save resumeA >"$TMPDIR/save.txt" 2>&1
check "the snapshot holds the reported command" "+resume" "$IMUX_STATE_DIR/snapshots/resumeA.imux"
check "and its arguments" "31339" "$IMUX_STATE_DIR/snapshots/resumeA.imux"
wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1
pkill -f "sleep 41" 2>/dev/null
ARGS="new -s other" HOLD=9 client "$TMPDIR/j.out" >/dev/null &
CLIENT=$!
sleep 3
$BIN restore resumeA >"$TMPDIR/rest.txt" 2>&1
sleep 4
assert "the reported command runs again in the restored pane" 'pgrep -f "sleep 31339" >/dev/null'
wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1
pkill -f "sleep 31339" 2>/dev/null
pkill -f "sleep 41" 2>/dev/null
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
