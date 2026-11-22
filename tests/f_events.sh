#!/bin/bash
# The event stream and agent prompt (M38): `imux events` (lines, the types filter, bad types, a
# reader that stalls is told it lost events and the server goes on), `imux api version`, and
# `imux agent prompt` against a fake agent (refused while agent-control is off, typed when on,
# --wait, refused while blocked or without an agent record, refused from inside a pane unless
# command-from-panes is on).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-events.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0
T="$TMPDIR"
LOG="$T/imux-$(id -u)/server.log"
export T

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

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

echo "-- api version"
assert "api version needs no server" '[ "$($BIN api version)" = "api=1" ]'

$BIN new-session -d -s a -n first >/dev/null 2>&1

echo "-- the stream"
$BIN events >"$T/all.out" 2>"$T/all.err" &
ALL=$!
$BIN events --types window-renamed,after-new-window >"$T/some.out" 2>&1 &
SOME=$!
$BIN events --types agent-state-changed >"$T/agent.out" 2>&1 &
AGENTEV=$!
assert "a subscription starts with a hello line" 'until_true "grep -aq subscribed $T/all.out" 40 && grep -aq "\"api\":1" $T/all.out'

$BIN rename-window -t a:0 "named \"one\"" >/dev/null 2>&1
$BIN new-window -d -t a -n second >/dev/null 2>&1
assert "a rename is an event with its names" 'until_true "grep -aq window-renamed $T/all.out" 40 && grep -a window-renamed $T/all.out | grep -aq "named \\\\\"one\\\\\""'
assert "every line is one JSON object with seq and time" 'grep -a window-renamed $T/all.out | grep -aqE "^\{\"event\":\"window-renamed\",\"seq\":[0-9]+,\"time\":[0-9]{13}"'
assert "the session and window ids are there" 'grep -a window-renamed $T/all.out | grep -aq "\"session\":\"\\$"'
assert "no types: the after- events are not sent" '! grep -aq "\"event\":\"after-" $T/all.out'
assert "a new window is an event" 'until_true "grep -aq window-linked $T/all.out" 40'
assert "named types only" 'until_true "grep -aq window-renamed $T/some.out" 40 && ! grep -aq window-linked $T/some.out'
assert "an after- event when asked for by name" 'until_true "grep -aq \"\\\"event\\\":\\\"after-new-window\\\"\" $T/some.out" 40'
assert "the sequence goes up" '[ "$(grep -ao "\"seq\":[0-9]*" $T/all.out | cut -d: -f2 | sort -n | uniq | wc -l | tr -d " ")" = "$(grep -ao "\"seq\":[0-9]*" $T/all.out | cut -d: -f2 | wc -l | tr -d " ")" ]'

$BIN events --types nonsense >"$T/bad.out" 2>&1
assert "an unknown type is refused" 'grep -aq "unknown event type" $T/bad.out'
$BIN events --nope >"$T/bad2.out" 2>&1
assert "a bad flag shows the usage" 'grep -aq "usage: imux events" $T/bad2.out'

echo "-- a reader that stalls"
$BIN events --types after-set-option 2>/dev/null | sleep 100 &
STALL=$!
sleep 1
pad=$(printf 'x%.0s' $(seq 1 200))
: >"$T/flood.conf"
# The server drops a subscriber once 64 KB is queued behind the kernel's socket buffers and the pipe.
# macOS buffers are small (3000 events, 320 KB, are plenty there); Linux's default 208 KB per socket
# swallows that much (measured: 3000 events do not trigger the drop, 4000 do), so it gets four times more.
case "$(uname -s)" in
  Linux) flood=12000 ;;
  *) flood=3000 ;;
esac
for i in $(seq 1 $flood); do echo "set -g @pad$i $pad" >>"$T/flood.conf"; done
$BIN source-file "$T/flood.conf" >/dev/null 2>&1
assert "a subscriber that does not read is told and dropped" 'until_true "grep -aq \"does not read\" $LOG" 40'
assert "the server goes on" '$BIN has-session -t a'
$BIN rename-window -t a:0 after-the-flood >/dev/null 2>&1
assert "and the others did not notice" 'until_true "grep -aq after-the-flood $T/all.out" 40 && ! grep -aq events_lost $T/all.out'
kill "$STALL" 2>/dev/null

echo "-- agent prompt"
cat >"$T/agent.sh" <<'EOF'
#!/bin/bash
n=0
r() { n=$((n + 1)); "$BIN" pane report-agent --source fake --agent fake --state "$1" --seq "$n" >/dev/null 2>&1; }
r idle
: >"$T/agent.ready"
while IFS= read -r line </dev/tty; do
  printf '%s\n' "$line" >>"$T/agent.got"
  case "$line" in
    *block*) r blocked ;;
    *) r working; sleep 1; r done ;;
  esac
done
EOF
chmod +x "$T/agent.sh"
AP=$($BIN new-window -d -P -F '#{pane_id}' -t a -n agent | tr -d '%')
PP=$($BIN new-window -d -P -F '#{pane_id}' -t a -n plain | tr -d '%')
sleep 1
$BIN send-keys -t "%$AP" "$T/agent.sh" Enter >/dev/null 2>&1
assert "the fake agent is up" 'until_true "[ -f $T/agent.ready ]" 40'
assert "its state is known" 'until_true "$BIN agent get $AP | grep -aq \"state: idle\"" 40'

$BIN agent prompt "$AP" hello >"$T/p1.out" 2>&1
assert "refused while agent-control is off" 'grep -aq "agent-control" $T/p1.out && [ ! -f $T/agent.got ]'
$BIN set-option -g agent-control on >/dev/null 2>&1
$BIN agent prompt "$AP" hello world >"$T/p2.out" 2>&1
assert "typed when it is on" 'grep -aq "^sent" $T/p2.out && until_true "grep -aqx \"hello world\" $T/agent.got" 40'
assert "the agent took it up" 'until_true "$BIN agent get $AP | grep -aq \"state: working\"" 40'
sleep 2
began=$SECONDS
$BIN agent prompt --wait --timeout 20 "$AP" second >"$T/p3.out" 2>&1
took=$((SECONDS - began))
assert "--wait returns when the turn is over, not at once (took ${took}s, the turn lasts 1s)" 'grep -aqx done $T/p3.out && grep -aqx second $T/agent.got && [ "$took" -ge 1 ]'
$BIN agent prompt --wait --timeout 1 "$AP" please block >"$T/p3b.out" 2>&1
assert "--wait also ends in blocked" 'grep -aqx blocked $T/p3b.out'
$BIN agent prompt "$AP" more >"$T/p4.out" 2>&1
assert "refused while the agent is blocked" 'grep -aq "blocked" $T/p4.out && ! grep -aqx more $T/agent.got'
$BIN agent prompt "$PP" hello >"$T/p5.out" 2>&1
assert "refused for a pane without an agent" 'grep -aq "no agent record" $T/p5.out'
$BIN agent prompt 999 hello >"$T/p5b.out" 2>&1
assert "refused for a pane that is not there" 'grep -aq "no pane 999" $T/p5b.out'
$BIN agent prompt "$AP" >"$T/p5c.out" 2>&1
assert "no text shows the usage" 'grep -aq "usage: imux agent prompt" $T/p5c.out'
big=$(printf 'y%.0s' $(seq 1 9000))
$BIN agent prompt "$AP" "$big" >"$T/p5d.out" 2>&1
assert "text over 8 KiB is refused" 'grep -aq "longer than" $T/p5d.out'

$BIN set-option -g command-from-panes off >/dev/null 2>&1
$BIN send-keys -t "%$PP" "$BIN agent prompt $AP from-pane > $T/p6.out 2>&1" Enter >/dev/null 2>&1
assert "refused from inside a pane" 'until_true "grep -aq \"inside a pane\" $T/p6.out" 40'
$BIN set-option -g command-from-panes on >/dev/null 2>&1
$BIN send-keys -t "%$PP" "$BIN agent prompt $AP from-pane > $T/p7.out 2>&1" Enter >/dev/null 2>&1
assert "from inside a pane it gets as far as the agent state when command-from-panes is on" 'until_true "grep -aq blocked $T/p7.out" 40'
assert "the state changes of the fake agent were events" 'grep -aq "\"state\":\"blocked\"" $T/agent.out && grep -aq "\"state\":\"working\"" $T/agent.out'

kill "$ALL" "$SOME" "$AGENTEV" 2>/dev/null
$BIN kill-server >/dev/null 2>&1
assert "no errors in the server log" '! grep -a "\"level\":\"error\"" $LOG'
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
