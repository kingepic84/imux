#!/bin/bash
# P10.7, `imux agent start NAME [--kind K] [--pane P] [--timeout MS] [-- args...]`: the command comes
# from worktree-command or the agent-kind table (never from the caller), arguments are quoted for
# the pane's shell, the pane has to be at its prompt with no agent, ready is the first idle report,
# a blocked start is not_ready and keeps its name, a silent program times out and loses the name,
# and the rules for a program in a pane.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qas.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
FIXDIR="$(cd "$(dirname "$0")" && pwd)/fixtures"
export PATH="$FIXDIR:$PATH"
export HOME="$TMPDIR/home"
mkdir -p "$HOME"
printf 'PATH="%s:$PATH"\nexport PATH\n' "$FIXDIR" >"$HOME/.profile"
export FAKE_IMUX="$BIN"
export FAKE_ARGS_FILE="$TMPDIR/args"
HOLDER="$FIXDIR/pane_holder.sh"
fail=0
seq=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 400; $BIN kill-server >/dev/null 2>&1; kill $$ >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\nset -g default-size 100x30\nset -g agent-control on\nset -g restore-commands "claude fake_agent.sh"\nset -g agent-kind "fake=fake_agent.sh bad=rm"\nset -g worktree-command fake_agent.sh\n' >"$IMUX_CONFIG"

run() {
  OUT="$TMPDIR/out"
  ERR="$TMPDIR/err"
  timeout 60 $BIN "$@" >"$OUT" 2>"$ERR" </dev/null
  RC=$?
}

newpane() {
  $BIN new-window -d -P -F '#{pane_id}' -t as 2>/dev/null
}

inpane() {
  local pane=$1
  shift
  seq=$((seq + 1))
  rm -f "$TMPDIR/rc$seq"
  (trap '' PIPE; printf '%s\n' "{ $*; } >$TMPDIR/o$seq 2>&1; echo \$? >$TMPDIR/rc$seq" >"$TMPDIR/fifo$pane") >/dev/null 2>&1 &
  local i
  for i in $(seq 1 100); do
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

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s as >/dev/null 2>&1
sleep 1

echo "-- the default kind (worktree-command)"
run agent start alpha --pane %0
assert "start answers ready and the pane" '[ $RC = 0 ] && grep -aqx "ready %0" "$OUT"'
run agent get alpha
assert "the agent has the name and is idle" '[ $RC = 0 ] && grep -aqx "name: alpha" "$OUT" && grep -aqx "state: idle" "$OUT"'
assert "no arguments reached it" '[ ! -s "$FAKE_ARGS_FILE" ] || [ "$(wc -l <"$FAKE_ARGS_FILE")" -le 1 ]'

echo "-- a kind from agent-kind, --json"
P1=$(newpane)
sleep 0.5
run agent start beta --kind fake --pane "$P1" --json
assert "the JSON has pane, name and state" '[ $RC = 0 ] && grep -aqE "^\{\"pane\":\"$P1\",\"name\":\"beta\",\"state\":\"idle\"(,\"seq\":[0-9]+)?\}$" "$OUT"'
run agent get beta --json
assert "agent get finds it by name" '[ $RC = 0 ] && grep -aq "\"pane\":\"$P1\"" "$OUT"'

echo "-- arguments are quoted for the pane shell"
P2=$(newpane)
sleep 0.5
rm -f "$TMPDIR/PWNED" "$TMPDIR/PWNED2"
run agent start hostile --kind fake --pane "$P2" -- 'a b' "it's" "\$(touch $TMPDIR/PWNED)" ";touch $TMPDIR/PWNED2" '--mode' 'x'
assert "start succeeds" '[ $RC = 0 ]'
assert "each argument arrived as one word" 'grep -Fxq "a b" "$FAKE_ARGS_FILE" && grep -Fxq "it'"'"'s" "$FAKE_ARGS_FILE" && grep -Fxq "\$(touch $TMPDIR/PWNED)" "$FAKE_ARGS_FILE" && grep -Fxq ";touch $TMPDIR/PWNED2" "$FAKE_ARGS_FILE" && grep -Fxq "x" "$FAKE_ARGS_FILE"'
assert "nothing was executed from them" '[ ! -e "$TMPDIR/PWNED" ] && [ ! -e "$TMPDIR/PWNED2" ]'
run agent start ctl --kind fake --pane "$P2" --json -- "$(printf 'a\tb')"
assert "a control character in an argument is refused (and this pane has an agent anyway)" '[ $RC = 1 ]'
P2B=$(newpane)
sleep 0.5
run agent start ctl --kind fake --pane "$P2B" --json -- "$(printf 'a\tb')"
assert "a control character is invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR"'
run agent start many --kind fake --pane "$P2B" --json -- $(seq 1 70)
assert "more than 64 arguments are invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR"'

echo "-- the pane must be at its prompt, with no agent"
P3=$(newpane)
sleep 0.5
$BIN send-keys -t "$P3" -l "sleep 600" >/dev/null 2>&1
$BIN send-keys -t "$P3" Enter >/dev/null 2>&1
sleep 1
run agent start busy --pane "$P3" --json
assert "a busy pane is not_ready" '[ $RC = 1 ] && grep -aq "\"code\":\"not_ready\"" "$ERR" && grep -aq "not at its shell prompt" "$ERR"'
run agent get busy --json
assert "and the name was not taken" '[ $RC = 1 ] && grep -aq "not_found" "$ERR"'
run agent start again --pane %0 --json
assert "a pane that has an agent is not_ready" '[ $RC = 1 ] && grep -aq "\"code\":\"not_ready\"" "$ERR" && grep -aq "already has an agent" "$ERR"'
run agent start nopane --pane %999 --json
assert "a missing pane is not_found" '[ $RC = 1 ] && grep -aq "\"code\":\"not_found\"" "$ERR"'

echo "-- names and kinds"
P4=$(newpane)
sleep 0.5
run agent start alpha --pane "$P4" --json
assert "a name in use is invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR" && grep -aq "already taken" "$ERR"'
for bad in Bad 1a a.b "a:b" -x; do
  run agent start "$bad" --pane "$P4" --json
  assert "the name '$bad' is refused" '[ $RC != 0 ]'
done
run agent start nokind --kind nosuch --pane "$P4" --json
assert "a kind that is not in agent-kind is invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR" && grep -aq "no kind nosuch" "$ERR"'
run agent start nokind --kind bad --pane "$P4" --json
assert "a kind whose program is not in restore-commands is permission_denied" '[ $RC = 1 ] && grep -aq "\"code\":\"permission_denied\"" "$ERR" && grep -aq "restore-commands" "$ERR"'
run capture-pane -p -t "$P4"
assert "and nothing was typed" '! grep -aq "rm" "$OUT"'
run agent start t1 --pane "$P4" --timeout 1000 --json
assert "a timeout under 3000 is invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR"'
run agent start t1 --pane "$P4" --timeout 999999 --json
assert "a timeout over 300000 is invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR"'
run agent start t1 --pane "$P4" --timeout x
assert "a timeout that is not a number is a usage error" '[ $RC = 2 ]'
$BIN set -g worktree-command "" >/dev/null 2>&1
run agent start empty --pane "$P4" --json
assert "no kind and an empty worktree-command is invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR" && grep -aq "worktree-command" "$ERR"'
$BIN set -g worktree-command fake_agent.sh >/dev/null 2>&1
$BIN set -g agent-control off >/dev/null 2>&1
run agent start nocontrol --pane "$P4" --json
assert "with agent-control off it is permission_denied" '[ $RC = 1 ] && grep -aq "\"code\":\"permission_denied\"" "$ERR" && grep -aq "agent-control" "$ERR"'
$BIN set -g agent-control on >/dev/null 2>&1
run capture-pane -p -t "$P4"
assert "nothing was typed into the pane through all of these" '! grep -aq "fake_agent" "$OUT"'

echo "-- a blocked start is not_ready and keeps the name"
run agent start gamma --kind fake --pane "$P4" --json -- --mode blocked
assert "blocked at startup is not_ready" '[ $RC = 1 ] && grep -aq "\"code\":\"not_ready\"" "$ERR" && grep -aq "blocked at startup" "$ERR"'
run agent get gamma
assert "the agent keeps the name and is blocked" '[ $RC = 0 ] && grep -aqx "state: blocked" "$OUT"'

echo "-- a silent program times out and loses the name"
P5=$(newpane)
sleep 0.5
run agent start quiet --kind fake --pane "$P5" --timeout 3000 --json -- --mode late
assert "no report in time is timeout and says why" '[ $RC = 1 ] && grep -aq "\"code\":\"timeout\"" "$ERR" && grep -aq "never the screen" "$ERR"'
run agent get quiet --json
assert "and the name is not kept" '[ $RC = 1 ] && grep -aq "not_found" "$ERR"'
sleep 4
run agent list --json
assert "the program reports after the timeout" '[ $RC = 0 ] && grep -aq "\"pane\":\"$P5\"" "$OUT"'
run agent get quiet --json
assert "and the name does not come back" '[ $RC = 1 ] && grep -aq "not_found" "$ERR"'
run agent start quiet --pane "$P5" --json
assert "the pane now has an agent" '[ $RC = 1 ] && grep -aq "already has an agent" "$ERR"'

echo "-- the client leaving ends the wait, not the program"
P6=$(newpane)
sleep 0.5
timeout 2 $BIN agent start leaver --kind fake --pane "$P6" --timeout 60000 -- --mode late >/dev/null 2>&1 </dev/null
sleep 1
run list-panes -a
assert "the server is still there" '[ $RC = 0 ]'
run agent start other --pane "$P6" --json
assert "the pane is busy running the program" '[ $RC = 1 ] && grep -aq "not_ready" "$ERR"'
sleep 4
run agent get leaver
assert "the first report gives the pane the name it was started with" '[ $RC = 0 ] && grep -aqx "name: leaver" "$OUT"'

echo "-- from a pane"
P7=$(newpane)
sleep 0.5
hold "${P7#%}"
P8=$(newpane)
sleep 0.5
inpane "${P7#%}" "$BIN agent start x1 --kind fake --pane $P8 --json"
assert "a program in a user-made pane may not start an agent in a pane it did not spawn" '[ "$RC" != 0 ] && grep -aq "permission_denied" "$O"'
inpane "${P7#%}" "$BIN agent start x1 --kind fake --json"
assert "from a pane the pane must be named" '[ "$RC" != 0 ] && grep -aq "invalid_argument" "$O" && grep -aq "which pane" "$O"'
inpane "${P7#%}" "$BIN agent start x1 --kind fake --pane $P7 --json"
assert "never its own pane" '[ "$RC" != 0 ] && grep -aq "permission_denied" "$O"'
inpane "${P7#%}" "$BIN new-window -d -P -F '#{pane_id}' -t as"
SPAWNED=$(tr -d '\n' <"$O")
assert "it can spawn a pane" 'printf "%s" "$SPAWNED" | grep -aq "^%[0-9][0-9]*$"'
sleep 1
inpane "${P7#%}" "$BIN agent start x2 --kind fake --pane $SPAWNED --json"
assert "and start an agent in the pane it spawned" '[ "$RC" = 0 ] && grep -aq "\"name\":\"x2\"" "$O"'
run agent get x2 --json
assert "that agent has the name" '[ $RC = 0 ] && grep -aq "\"pane\":\"$SPAWNED\"" "$OUT"'
$BIN set -g command-from-panes off >/dev/null 2>&1
P9=$(newpane)
sleep 0.5
inpane "${P7#%}" "$BIN agent start x3 --kind fake --pane $P9 --json"
assert "with command-from-panes off it only looks" '[ "$RC" != 0 ]'
$BIN set -g command-from-panes spawned >/dev/null 2>&1

if [ "$fail" != 0 ]; then
  echo "--- diagnostics"
  $BIN agent list
  $BIN list-panes -a -F '#{session_name} #{pane_id} #{agent_name}'
fi

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
