#!/bin/bash
# The command line of imux itself (src/cli.rs): every form runs without a panic, with and without a
# server; usage errors say why and show the usage and exit 2; --help works; the words that belong
# to the server (new -d, a lone agent, tmux-style commands) still reach it.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qcli.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

# Run the words, keep the output in $O and the status in $RC.
run() {
  O="$TMPDIR/out.$$"
  timeout 20 $BIN "$@" >"$O" 2>&1 </dev/null
  RC=$?
}
quiet() { ! grep -aq "panicked\|panic:\|Mismatch between definition" "$O"; }

$BIN kill-server >/dev/null 2>&1

echo "-- no server"
for form in "--version" "-V" "version" "api version" "ls" "list-keys" "snapshots" "snapshots --history" "doctor" "completions zsh" "init zsh" \
  "check-config --summary" "agent list" "agent get 1" "agent wait 1 --timeout 1" "agent prompt 1 hello" "plugin list" "worktree list" \
  "save x" "restore x" "delete-snapshot x" "events --types a,b" "pane release-agent 1 --source s" "pane report-agent 1 --source s --agent a --state idle -- cmd arg"; do
  run $form
  assert "no panic: $form" 'quiet'
done
run --version
assert "--version prints the version" 'grep -aqx "imux [0-9.]*" "$O" && [ $RC = 0 ]'
run api version
assert "api version prints api=N" 'grep -aq "^api=[0-9]*$" "$O" && [ $RC = 0 ]'

echo "-- help"
run --help
assert "--help lists the commands and the server's ones" '[ $RC = 0 ] && grep -aq "Commands:" "$O" && grep -aq "split-window" "$O" && grep -aq "agent" "$O"'
assert "and never names another multiplexer" '! grep -aqi "tmux" "$O"'
run agent wait --help
assert "agent wait --help shows its flags" '[ $RC = 0 ] && grep -aq -- "--until" "$O" && grep -aq -- "--timeout" "$O"'
run attach --help
assert "attach --help shows -r" '[ $RC = 0 ] && grep -aq -- "-r" "$O"'
run -h
assert "-h is --help" '[ $RC = 0 ] && grep -aq "Commands:" "$O"'

echo "-- usage errors"
run events --bogus
assert "events --bogus: reason, usage, exit 2" '[ $RC = 2 ] && grep -aq "unexpected argument" "$O" && grep -aq "usage: imux events" "$O"'
run agent prompt 3
assert "agent prompt without text: usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux agent prompt" "$O"'
run agent wait
assert "agent wait without a pane: usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux agent wait" "$O"'
run attach -t w --bogus
assert "attach with an unknown flag: exit 2" '[ $RC = 2 ] && grep -aq "unexpected argument" "$O"'
run worktree
assert "worktree without a name: usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: worktree" "$O"'
run worktree list a b
assert "worktree list with two words: usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: worktree list" "$O"'
run pane report-agent --source s
assert "pane report-agent missing flags: exit 2" '[ $RC = 2 ] && grep -aq "usage: imux pane report-agent" "$O"'
run completions
assert "completions without a shell: exit 2" '[ $RC = 2 ]'
run pane
assert "a bare pane prints its help, exit 2" '[ $RC = 2 ] && quiet'
run plugin
assert "a bare plugin prints its help, exit 2" '[ $RC = 2 ] && quiet'
run api
assert "a bare api prints its help, exit 2" '[ $RC = 2 ] && quiet'
run nonsense
assert "an unknown command prints the usage, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux \[new" "$O"'
run completions nosuchshell
assert "a bad shell name is an error, not a panic" '[ $RC != 0 ] && quiet'
run pane report-agent --source s --agent a --state idle
assert "report-agent outside a pane says there is no pane" '[ $RC != 0 ] && grep -aq "IMUX_PANE_ID" "$O"'

echo "-- with a server"
$BIN new-session -d -s w >/dev/null 2>&1
sleep 1
for form in "ls" "list-keys" "snapshots" "snapshots --history" "agent list" "agent get 1" "plugin list" "worktree list" "doctor" "events --types nothing" \
  "check-config --summary" "save cli1" "snapshots" "delete-snapshot cli1" "agent wait 1 --timeout 1" "agent prompt 1 hello"; do
  run $form
  assert "no panic: $form" 'quiet'
done
run ls
assert "ls shows the session" 'grep -aq "^w" "$O" && [ $RC = 0 ]'
run save cli2
assert "save then snapshots lists it" '$BIN snapshots | grep -aq cli2'
run delete-snapshot cli2
assert "delete-snapshot removes it" '! $BIN snapshots | grep -aq cli2'
run agent get 99
assert "agent get of a pane that is not there is an error" '[ $RC != 0 ] && quiet'

echo "-- the words that are the server's"
run new -d -s viaserver
assert "new -d is the new-session command" '[ $RC = 0 ] && $BIN has-session -t =viaserver'
run list-sessions -F '#{session_name}'
assert "a command the server runs works as a word" 'grep -aqx w "$O" && grep -aqx viaserver "$O"'
run agent
assert "a lone agent is the server's command, not a panic" 'quiet'
run agent -t nothing
assert "agent -t x is the server's too" 'quiet && ! grep -aq "usage: imux agent" "$O"'
run split-window -h -d -t w: -P -F '#{pane_id}'
assert "tmux-style flags such as -h reach the server untouched" '[ $RC = 0 ] && grep -aq "^%" "$O"'
run list-keys -T prefix
assert "list-keys -T is the server's command, not a flag error" '[ $RC = 0 ] && grep -aq -- "-T prefix" "$O" && quiet'
run list-keys -T nonsense
assert "and its own errors come back" '[ $RC != 0 ] && grep -aq "unknown key table" "$O"'
run -L other ls
assert "-L is taken before the tree" 'grep -aq "no server running" "$O"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
