#!/bin/bash
# P10.4, `imux pane run PANE text...` (one ordered write: the text as a paste, then Enter, with no
# agent needed), `imux agent read PANE [--lines N] [--ansi]` (the recent text of a pane), the rules
# for a program in a pane (only the panes it spawned), the agent rules (agent-control, a blocked
# agent), and `send-keys` writing nothing when a word of it is refused.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qpr.XXXXXX)
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

run() {
  OUT="$TMPDIR/out"
  ERR="$TMPDIR/err"
  timeout 20 $BIN "$@" >"$OUT" 2>"$ERR" </dev/null
  RC=$?
}

waitfile() {
  local i
  for i in $(seq 1 50); do
    [ -f "$1" ] && grep -aq -- "$2" "$1" && return 0
    sleep 0.2
  done
  return 1
}

inpane() {
  shift
  seq=$((seq + 1))
  rm -f "$TMPDIR/rc$seq"
  (trap '' PIPE; printf '%s\n' "{ $*; } >$TMPDIR/o$seq 2>&1; echo \$? >$TMPDIR/rc$seq" >"$TMPDIR/fifo") >/dev/null 2>&1 &
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
$BIN new-session -d -s pr >/dev/null 2>&1
sleep 1
mkfifo "$TMPDIR/fifo"
$BIN send-keys -t %0 -l "sh $HOLDER $TMPDIR/fifo" >/dev/null 2>&1
$BIN send-keys -t %0 Enter >/dev/null 2>&1
for i in $(seq 1 50); do
  [ -f "$TMPDIR/ready" ] && break
  (trap '' PIPE; printf '%s\n' "echo >$TMPDIR/ready" >"$TMPDIR/fifo") >/dev/null 2>&1 &
  sleep 0.2
done
$BIN split-window -d >/dev/null 2>&1
sleep 1

echo "-- typing a line into a pane"
run pane run 1 "echo ran-\$((1+1)) >$TMPDIR/f1"
assert "pane run answers sent" '[ $RC = 0 ] && grep -aqx "sent" "$OUT"'
assert "the shell in the pane ran the line" 'waitfile "$TMPDIR/f1" "^ran-2$"'
run pane run 1 "echo A >>$TMPDIR/f2"
run pane run 1 "echo B >>$TMPDIR/f2"
waitfile "$TMPDIR/f2" "^B$"
assert "two calls run in the order they were made" '[ "$(tr -d "\n" <"$TMPDIR/f2")" = AB ]'
run pane run 1 "echo L1 >>$TMPDIR/f3
echo L2 >>$TMPDIR/f3"
waitfile "$TMPDIR/f3" "^L2$"
assert "a line break in the text is another Enter" '[ "$(tr -d "\n" <"$TMPDIR/f3")" = L1L2 ]'
printf 'echo S >%s\n' "$TMPDIR/f4" | timeout 20 $BIN pane run 1 - >/dev/null 2>&1
assert "a lone - reads the standard input" 'waitfile "$TMPDIR/f4" "^S$"'
run pane run 1 --json "echo J >$TMPDIR/f5"
assert "--json says which pane and that it was sent" '[ $RC = 0 ] && grep -aqE "^\{\"pane\":\"%1\",\"sent\":true(,\"seq\":[0-9]+)?\}$" "$OUT"'
waitfile "$TMPDIR/f5" "^J$"
run pane run 1 -- "-n echo-dash >$TMPDIR/f6"
assert "after -- the text may start with a dash" '[ $RC = 0 ]'

echo "-- the text is a paste"
printf '%s\n' 'case "$2" in on) printf "\033[?2004h";; esac' 'while IFS= read -r l; do printf "%s\n" "$l" >>"$1"; done' >"$TMPDIR/bp.sh"
$BIN split-window -d -P -F '#{pane_id}' "sh $TMPDIR/bp.sh $TMPDIR/bp-on on" >"$TMPDIR/p-on" 2>&1
$BIN split-window -d -P -F '#{pane_id}' "sh $TMPDIR/bp.sh $TMPDIR/bp-off off" >"$TMPDIR/p-off" 2>&1
PON=$(head -1 "$TMPDIR/p-on" | tr -d '%')
POFF=$(head -1 "$TMPDIR/p-off" | tr -d '%')
sleep 1
run pane run "$PON" hello
run pane run "$POFF" hello
ESC=$(printf '\033')
assert "a program that asked for bracketed paste gets the markers" 'waitfile "$TMPDIR/bp-on" "hello" && LC_ALL=C grep -aq "${ESC}\[200~hello${ESC}\[201~" "$TMPDIR/bp-on"'
assert "any other program gets the bare text" 'waitfile "$TMPDIR/bp-off" "hello" && grep -aqx "hello" "$TMPDIR/bp-off"'
run pane run "$POFF" "$(printf 'x\033[201~y\003z')"
assert "escape and control characters are not typed" 'waitfile "$TMPDIR/bp-off" "xyz" || grep -aq "x.*y.*z" "$TMPDIR/bp-off"; ! LC_ALL=C grep -aq "${ESC}" "$TMPDIR/bp-off"'

echo "-- refusals"
run pane run 99 --json "echo x"
assert "a missing pane is not_found on stderr, exit 1" '[ $RC = 1 ] && [ ! -s "$OUT" ] && grep -aq "\"code\":\"not_found\"" "$ERR"'
run pane run 99 "echo x"
assert "plain: the text without the code" '[ $RC = 1 ] && grep -aqx "imux: no pane 99" "$OUT"'
run pane run 1 --json "   "
assert "nothing to send is invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR"'
BIG=$(head -c 9000 /dev/zero | tr '\0' a)
run pane run 1 --json "$BIG"
assert "text over 8 KiB is limit_reached" '[ $RC = 1 ] && grep -aq "\"code\":\"limit_reached\"" "$ERR"'
run pane run
assert "a bare pane run prints its usage and exits 2" '[ $RC = 2 ]'
run pane run 1
assert "a pane without text is a usage error, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux pane run pane" "$OUT"'

echo "-- a pane that runs an agent"
inpane 0 "$BIN pane report-agent --source t --agent fake --state idle"
assert "the holder pane reported" '[ "$RC" = 0 ]'
run pane run 0 --json "echo x"
assert "typing into an agent pane needs agent-control" '[ $RC = 1 ] && grep -aq "\"code\":\"permission_denied\"" "$ERR" && grep -aq "agent-control" "$ERR"'
$BIN set -g agent-control on >/dev/null 2>&1
run pane run 0 "echo x"
assert "with agent-control on it is typed" '[ $RC = 0 ] && grep -aqx "sent" "$OUT"'
inpane 0 "$BIN pane report-agent --source t --agent fake --state blocked --message ok"
run pane run 0 --json "echo x"
assert "a blocked agent is refused, the text would answer it" '[ $RC = 1 ] && grep -aq "\"code\":\"agent_blocked\"" "$ERR"'
$BIN set -g agent-control off >/dev/null 2>&1

echo "-- reading a pane"
run pane run 1 "echo MARK-\$((20+22))"
waitfile "$TMPDIR/f1" "ran-2"
sleep 0.5
run agent read 1
assert "agent read prints the text of the pane" '[ $RC = 0 ] && grep -aq "MARK-42" "$OUT"'
run agent read 1 --lines 1
assert "--lines 1 is one line" '[ $RC = 0 ] && [ "$(wc -l <"$OUT" | tr -d " ")" = 1 ]'
run agent read 1 --lines 3
assert "--lines 3 is at most three" '[ $RC = 0 ] && [ "$(wc -l <"$OUT" | tr -d " ")" -le 3 ]'
run agent read 1
assert "the default is at most 100 lines" '[ "$(wc -l <"$OUT" | tr -d " ")" -le 100 ]'
run agent read 1 --lines 5 --json
assert "--json has the pane, the count and the text" '[ $RC = 0 ] && grep -aq "^{\"pane\":\"%1\",\"lines\":5,\"text\":\"" "$OUT" && [ "$(wc -l <"$OUT" | tr -d " ")" = 1 ]'
run pane run 1 "printf '\\033[31mred\\033[0m\\n'"
sleep 0.7
run agent read 1 --lines 4
assert "plain text has no escape sequences" 'grep -aq "red" "$OUT" && ! LC_ALL=C grep -aq "${ESC}" "$OUT"'
run agent read 1 --lines 6 --ansi
assert "--ansi keeps them" 'LC_ALL=C grep -aq "${ESC}\[[0-9;]*31m" "$OUT"'
run agent read 99 --json
assert "a missing pane is not_found" '[ $RC = 1 ] && grep -aq "\"code\":\"not_found\"" "$ERR"'
run agent read 1 --lines 0
assert "--lines 0 is a usage error" '[ $RC = 2 ]'
run agent read 1 --lines 10001
assert "--lines over 10000 is a usage error" '[ $RC = 2 ]'

echo "-- from a pane"
inpane 0 "$BIN new-window -d -P -F '#{pane_id}'"
KID=$(grep -a '^%' "$O" | head -1 | tr -d '%')
assert "the pane in the holder spawned a pane" '[ -n "$KID" ]'
sleep 1
inpane 0 "$BIN pane run $KID \"echo from-pane >$TMPDIR/f7\""
assert "it may type into the pane it spawned" '[ "$RC" = 0 ] && waitfile "$TMPDIR/f7" "^from-pane$"'
inpane 0 "$BIN pane run 1 \"echo no >$TMPDIR/f8\""
assert "it may not type into one it did not spawn" '[ "$RC" != 0 ] && grep -aq "not one this pane spawned" "$O"'
assert "and nothing was typed" '[ ! -f "$TMPDIR/f8" ]'
inpane 0 "$BIN agent read $KID --lines 3"
assert "it may read the pane it spawned" '[ "$RC" = 0 ]'
inpane 0 "$BIN agent read 1 --lines 3"
assert "it may not read another pane" '[ "$RC" != 0 ]'
$BIN set -g command-from-panes off >/dev/null 2>&1
inpane 0 "$BIN pane run $KID \"echo off >$TMPDIR/f9\""
assert "with command-from-panes off it only looks" '[ "$RC" != 0 ] && grep -aq "command-from-panes" "$O" && [ ! -f "$TMPDIR/f9" ]'
$BIN set -g command-from-panes spawned >/dev/null 2>&1

echo "-- send-keys writes nothing when it refuses"
$BIN send-keys -t %1 -H 7a 7a >/dev/null 2>&1
sleep 0.5
assert "control: valid hex bytes are typed" '$BIN capture-pane -p -t %1 | grep -aq "zz"'
$BIN send-keys -t %1 C-u >/dev/null 2>&1
$BIN send-keys -t %1 C-l >/dev/null 2>&1
sleep 0.5
$BIN send-keys -t %1 -H 71 71 5g >"$TMPDIR/sk1" 2>&1
assert "a bad hex byte is refused" 'grep -aq "hexadecimal" "$TMPDIR/sk1"'
$BIN send-keys -t %1 -N 0 qq >"$TMPDIR/sk2" 2>&1
assert "a bad repeat count is refused" 'grep -aq "1 to 1000" "$TMPDIR/sk2"'
sleep 0.5
assert "nothing of either was typed" '! $BIN capture-pane -p -t %1 | grep -aq "qq"'
$BIN send-keys -t %1 C-u >/dev/null 2>&1

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
