#!/bin/bash
# P10.1, who may do what from a pane: `command-from-panes off|spawned|on`, the record of who made a
# pane (`#{pane_spawned_by}`), a pane held to the panes it spawned, spawns that never take focus,
# `spawned-depth` and `spawned-panes-max`, the commands that stay refused below `on`, the prompt
# into agents, and the log line of each write.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qsp.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export IMUX_LOG=info
export IMUX_LOG_FILE="$TMPDIR/server.log"
export SHELL=/bin/sh
fail=0
seq=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 170; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\nset -g agent-control on\n' >"$IMUX_CONFIG"
$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s sp >/dev/null 2>&1
sleep 1

# Type a command into a pane (as the user would), keep its output in $O and its status in $RC.
inpane() {
  local pane=$1
  shift
  seq=$((seq + 1))
  rm -f "$TMPDIR/rc$seq"
  $BIN send-keys -t "$pane" -l "{ $*; } >$TMPDIR/o$seq 2>&1; echo \$? >$TMPDIR/rc$seq" >/dev/null 2>&1
  $BIN send-keys -t "$pane" Enter >/dev/null 2>&1
  local i
  for i in $(seq 1 50); do
    [ -f "$TMPDIR/rc$seq" ] && break
    sleep 0.2
  done
  O="$TMPDIR/o$seq"
  RC=$(cat "$TMPDIR/rc$seq" 2>/dev/null)
  [ -f "$O" ] || : >"$O"
}

active() { $BIN list-panes -F '#{pane_id} #{pane_active}' | awk '$2 == 1 { print $1 }'; }

echo "-- the default"
assert "a pane may act on the panes it spawned by default" '[ "$($BIN show-options -gv command-from-panes)" = spawned ]'
inpane %0 "$BIN send-keys -t %0 x"
assert "and no more than that: its own pane is not one it spawned" '[ "$RC" != 0 ] && grep -aq "command-from-panes spawned" "$O"'

echo "-- off: look only, as before"
$BIN set -g command-from-panes off >/dev/null 2>&1
inpane %0 "$BIN split-window -d -P -F '#{pane_id}'"
assert "a pane may not spawn while the option is off" '[ "$RC" != 0 ] && grep -aq "command-from-panes on" "$O"'
inpane %0 "$BIN list-panes -F '#{pane_id}'"
assert "it may look" '[ "$RC" = 0 ] && grep -aq "%0" "$O"'

echo "-- spawned: a pane acts on the panes it spawned"
$BIN set -g command-from-panes spawned >/dev/null 2>&1
assert "the option shows the value" '[ "$($BIN show-options -gv command-from-panes)" = spawned ]'
inpane %0 "$BIN split-window -P -F '#{pane_id}'"
assert "a pane can spawn a sibling" '[ "$RC" = 0 ] && grep -aqx "%1" "$O"'
assert "the spawn did not take the focus (no -d was typed)" '[ "$(active)" = "%0" ]'
assert "the record says who made it" '[ "$($BIN display-message -p -t %1 "#{pane_spawned_by}")" = "%0" ]'
assert "a pane the user made has no maker" '[ -z "$($BIN display-message -p -t %0 "#{pane_spawned_by}")" ]'
sleep 0.5
inpane %0 "$BIN send-keys -t %1 'echo ZZHELLO-FROM-0' Enter"
assert "it can type into the pane it spawned" '[ "$RC" = 0 ]'
sleep 0.8
assert "and the text arrived" '$BIN capture-pane -p -t %1 | grep -aq "^ZZHELLO-FROM-0"'
inpane %0 "$BIN capture-pane -p -t %1"
assert "it can read the pane it spawned" '[ "$RC" = 0 ] && grep -aq "ZZHELLO-FROM-0" "$O"'
inpane %0 "$BIN send-keys -t %0 x"
assert "it may not type into its own pane" '[ "$RC" != 0 ] && grep -aq "command-from-panes spawned" "$O"'
inpane %0 "$BIN respawn-pane -t %1"
assert "a command that passes the check can still fail on its own" '[ "$RC" != 0 ] && grep -aq "still running" "$O"'

echo "-- a pane the user made is not in the subtree"
$BIN new-window -d >/dev/null 2>&1
USER2=$($BIN list-panes -a -F '#{pane_id}' | tail -1)
assert "the user made another pane" '[ -n "$USER2" ] && [ "$USER2" != %0 ] && [ "$USER2" != %1 ]'
inpane %0 "$BIN send-keys -t $USER2 x"
assert "no typing into it" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
inpane %0 "$BIN kill-pane -t $USER2"
assert "no closing it" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
inpane %0 "$BIN capture-pane -p -t $USER2"
assert "no reading it" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
inpane %0 "$BIN split-window -t $USER2 -P -F '#{pane_id}'"
assert "no splitting it" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
UW=$($BIN display-message -p -t "$USER2" '#{window_id}')
inpane %0 "$BIN kill-window -t $UW"
assert "no closing its window" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
inpane %0 "$BIN send-keys -t '%1 %2' x"
assert "a target made of two ids is no target" '[ "$RC" != 0 ]'
inpane %0 "$BIN kill-pane -a -t %1"
assert "kill-pane -a is refused" '[ "$RC" != 0 ] && grep -aq "command-from-panes spawned" "$O"'

echo "-- refused below on, whatever the target"
for words in "set-option -g @x 1" "set-environment -g X 1" "bind-key z kill-server" "set-hook -g after-new-window ls" "source-file /dev/null" "pipe-pane -t %1 cat" "run-shell true" "if-shell true true" "select-pane -t %1" "select-window -t :0" "kill-session -t sp" "new-session -d -s other" "display-menu -T t a b c" "rename-window -t %1 x" "resize-pane -t %1 -x 10"; do
  inpane %0 "$BIN $words"
  assert "refused: $words" '[ "$RC" != 0 ] && ! grep -aq "^ok" "$O"'
done
assert "no option was set" '$BIN show-options -gv @x 2>&1 | grep -aq "unknown option"'
assert "no key was bound" '! $BIN list-keys | grep -aq "kill-server"'

echo "-- a pane that was spawned is held to its own subtree"
inpane %1 "$BIN split-window -P -F '#{pane_id}'"
assert "a spawned pane may spawn one level below" '[ "$RC" = 0 ] && grep -aqx "%[0-9]*" "$O"'
GRAND=$(head -1 "$O")
assert "the focus is still where the user left it" '[ "$(active)" = "%0" ]'
assert "the record chains" '[ "$($BIN display-message -p -t $GRAND "#{pane_spawned_by}")" = "%1" ]'
inpane %1 "$BIN send-keys -t %0 x"
assert "a child cannot type into its parent" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
inpane %1 "$BIN kill-pane -t %0"
assert "nor close it" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
inpane %1 "$BIN capture-pane -p -t %0"
assert "nor read it" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
inpane %1 "$BIN set-option -g @y 1"
assert "nor set an option" '[ "$RC" != 0 ]'
inpane $GRAND "$BIN split-window -P -F '#{pane_id}'"
assert "spawned-depth stops a third level" '[ "$RC" != 0 ] && grep -aq "spawned-depth" "$O"'
inpane %0 "$BIN send-keys -t $GRAND 'echo ZZDEEP' Enter"
assert "the first pane reaches its grandchild" '[ "$RC" = 0 ]'
sleep 0.8
assert "and the text arrived" '$BIN capture-pane -p -t $GRAND | grep -aq "^ZZDEEP"'
$BIN set -g spawned-depth 3 >/dev/null 2>&1
inpane $GRAND "$BIN new-window -P -F '#{pane_id}'"
assert "a larger spawned-depth allows a third level" '[ "$RC" = 0 ]'
GREAT=$(head -1 "$O")
assert "and the window did not become the current one" '[ "$($BIN display-message -p "#{window_index}")" = 0 ]'
$BIN set -g spawned-depth 2 >/dev/null 2>&1
inpane %0 "$BIN kill-pane -t $GREAT"
assert "the first pane closes what is below it" '[ "$RC" = 0 ]'
inpane %0 "$BIN kill-pane -t $GRAND"
assert "and its grandchild" '[ "$RC" = 0 ]'
sleep 0.5
assert "the pane is gone" '! $BIN list-panes -a -F "#{pane_id}" | grep -aqx "$GRAND"'

echo "-- spawned-panes-max"
$BIN set -g spawned-panes-max 2 >/dev/null 2>&1
inpane %0 "$BIN new-window -P -F '#{pane_id}'"
assert "a window is a spawn too, and takes no focus" '[ "$RC" = 0 ] && [ "$($BIN display-message -p "#{window_index}")" = 0 ]'
W2=$(head -1 "$O")
inpane %0 "$BIN new-window -P -F '#{pane_id}'"
assert "past the count it is refused and names the option" '[ "$RC" != 0 ] && grep -aq "spawned-panes-max" "$O"'
WID=$($BIN display-message -p -t "$W2" '#{window_id}')
inpane %0 "$BIN kill-window -t $WID"
assert "kill-window works on a window of spawned panes" '[ "$RC" = 0 ]'
sleep 0.5
inpane %0 "$BIN new-window -P -F '#{pane_id}'"
assert "closing one makes room" '[ "$RC" = 0 ]'
$BIN set -g spawned-panes-max 16 >/dev/null 2>&1

echo "-- agents"
inpane %1 "$BIN pane report-agent --source t --agent fake --state idle"
inpane "$USER2" "$BIN pane report-agent --source t --agent fake --state idle"
assert "both panes have an agent record" '$BIN agent get 1 | grep -aq fake && $BIN agent get "${USER2#%}" | grep -aq fake'
inpane %0 "$BIN agent prompt 1 hello"
assert "a prompt into a pane it spawned goes through" '[ "$RC" = 0 ]'
inpane %0 "$BIN agent prompt ${USER2#%} hello"
assert "a prompt into a pane it did not spawn is refused" '[ "$RC" != 0 ] && grep -aq "is not one this pane spawned" "$O"'
$BIN set -g agent-control off >/dev/null 2>&1
inpane %0 "$BIN agent prompt 1 hello"
assert "agent-control is still needed" '[ "$RC" != 0 ] && grep -aq "agent-control" "$O"'
$BIN set -g agent-control on >/dev/null 2>&1

echo "-- popups stay under their own option"
$BIN set -g popup-from-panes off >/dev/null 2>&1
inpane %0 "$BIN display-popup -E true"
assert "popup-from-panes off is refused and named" '[ "$RC" != 0 ] && grep -aq "popup-from-panes" "$O"'
$BIN set -g popup-from-panes on >/dev/null 2>&1

echo "-- on: a pane the user made can do what it could"
$BIN set -g command-from-panes on >/dev/null 2>&1
inpane %0 "$BIN set-option -g @fromzero 1"
assert "a user-made pane may configure" '[ "$RC" = 0 ] && [ "$($BIN show-options -gv @fromzero)" = 1 ]'
inpane %1 "$BIN set-option -g @fromone 1"
assert "a spawned pane is still held to its subtree" '[ "$RC" != 0 ] && $BIN show-options -gv @fromone 2>&1 | grep -aq "unknown option"'
inpane %1 "$BIN send-keys -t %0 x"
assert "and cannot reach its parent" '[ "$RC" != 0 ]'

echo "-- back to off: a spawned pane is look-only"
$BIN set -g command-from-panes off >/dev/null 2>&1
inpane %1 "$BIN split-window -P -F '#{pane_id}'"
assert "nothing is spawned" '[ "$RC" != 0 ] && grep -aq "command-from-panes on" "$O"'
inpane %0 "$BIN send-keys -t %1 x"
assert "a user-made pane is look-only again" '[ "$RC" != 0 ] && grep -aq "command-from-panes on" "$O"'

echo "-- doctor"
$BIN doctor >"$TMPDIR/doctor.off" 2>&1
assert "imux doctor names the tier and what it means" 'grep -aq "^pane commands: command-from-panes off: a pane may only look" "$TMPDIR/doctor.off"'
$BIN set -g command-from-panes spawned >/dev/null 2>&1
$BIN doctor >"$TMPDIR/doctor.sp" 2>&1
assert "and the spawn limits when the tier is spawned" 'grep -aq "^pane commands: command-from-panes spawned: .*at most 2 levels deep and 16 live panes" "$TMPDIR/doctor.sp"'
$BIN set -g command-from-panes off >/dev/null 2>&1

echo "-- the log"
assert "a write from a pane is logged with who, what and how it went" 'grep -aq "pane-write caller=%0 power=subtree command=split-window" "$IMUX_LOG_FILE"'
assert "the target is in the line" 'grep -aq "command=send-keys target=.*%1.* result=ok" "$IMUX_LOG_FILE"'
assert "a refusal is logged" 'grep -aq "refused set-option from pane 0" "$IMUX_LOG_FILE"'
assert "a failed write says so" 'grep -aq "result=failed" "$IMUX_LOG_FILE"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
