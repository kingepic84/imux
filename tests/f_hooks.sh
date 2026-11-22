#!/bin/bash
# Event hooks (M36): set-hook / show-hooks, after-<command>, events found by the server
# (sessions, windows, panes, clients, agents), arrays, the depth limit, and the rule that a name a
# pane could have set is never pasted into shell text.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-hooks.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
T="$TMPDIR"
EV="$T/ev.out"

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

seen() {
  until_true "grep -aqxF -- \"$1\" $EV" 40
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cat >"$IMUX_CONFIG" <<EOF
set -g automatic-rename off
set-hook -g session-created "run-shell 'echo session-created:\\\$IMUX_HOOK_SESSION_NAME >> $EV'"
set-hook -g after-new-window 'run-shell "echo after-new-window >> $EV"'
set-hook -g window-linked 'run-shell "echo window-linked >> $EV"'
set-hook -g window-renamed "run-shell 'echo window-renamed:\\\$IMUX_HOOK_WINDOW_NAME >> $EV'"
set-hook -ga window-renamed 'run-shell "echo second-command >> $EV"'
set-hook -g pane-exited 'run-shell "echo pane-exited >> $EV"'
set-hook -g pane-title-changed 'run-shell "echo bad:#{hook_window_name} >> $T/bad.out"'
set-hook -g session-renamed 'run-shell "echo session-renamed:#{q:hook_session_name} >> $EV"'
set-hook -g client-attached 'run-shell "echo client-attached >> $EV"'
set-hook -g client-detached 'run-shell "echo client-detached >> $EV"'
set-hook -g after-set-option 'run-shell "echo loop >> $T/loop.out" ; set -g display-panes-time 5'
set-hook -g worktree-created 'run-shell "echo worktree >> $EV"'
set-hook -g nonsense-event 'display-message x'
set-hook -g alert-bell 'display-message x'
EOF

echo "-- sessions, windows, panes"
$BIN new-session -d -s "h'1" -n "w one" >/dev/null 2>&1
assert "session-created fires, with the name in the environment" 'seen "session-created:h'"'"'1"'
assert "window-linked fires for its window" 'seen window-linked'
$BIN new-window -t "h'1" -n second >/dev/null 2>&1
assert "after-new-window fires for a command from the shell" 'seen after-new-window'
$BIN rename-window -t "h'1":1 "x y" >/dev/null 2>&1
assert "window-renamed gets the new name in the environment" 'seen "window-renamed:x y"'
assert "a second command of the same hook runs too (-a)" 'seen second-command'
$BIN rename-session -t "h'1" "r'2" >/dev/null 2>&1
assert "#{q:hook_session_name} is one quoted word" 'seen "session-renamed:r'"'"'2"'
$BIN send-keys -t "r'2":1 "printf '\\033]2;title\\007'" Enter
sleep 2
assert "a window name in shell text is refused, the command does not run" '[ ! -f $T/bad.out ]'
$BIN kill-pane -t "r'2":1 >/dev/null 2>&1
assert "pane-exited fires" 'seen pane-exited'

echo "-- show-hooks and set-hook -u"
$BIN show-hooks >"$T/hooks.txt" 2>&1
assert "show-hooks lists names with slots" 'grep -aq "^window-renamed\[0\] " $T/hooks.txt && grep -aq "^window-renamed\[1\] " $T/hooks.txt'
assert "names imux does not know are kept" 'grep -aq "^nonsense-event\[0\] " $T/hooks.txt && grep -aq "^alert-bell\[0\] " $T/hooks.txt'
$BIN set-hook -u "window-renamed[1]" >/dev/null 2>&1
$BIN show-hooks >"$T/hooks2.txt" 2>&1
assert "set-hook -u name[slot] removes one command" '! grep -aq "^window-renamed\[1\] " $T/hooks2.txt && grep -aq "^window-renamed\[0\] " $T/hooks2.txt'
$BIN set-hook -u pane-exited >/dev/null 2>&1
$BIN show-hooks >"$T/hooks3.txt" 2>&1
assert "set-hook -u name removes the hook" '! grep -aq "^pane-exited" $T/hooks3.txt'
$BIN set-hook -g "Bad Name" x >"$T/o1" 2>&1
assert "a bad hook name is an error" 'grep -aq "bad hook name" $T/o1'
$BIN set-hook -g pane-exited >"$T/o2" 2>&1
assert "set-hook without a command is an error" 'grep -aq "usage" $T/o2'

echo "-- a hook that sets an option fires itself: the depth limit stops it"
$BIN set -g display-time 1111 >/dev/null 2>&1
sleep 3
n=$(grep -c . "$T/loop.out" 2>/dev/null || echo 0)
assert "it ran, and not forever ($n runs)" '[ "$n" -ge 1 ] && [ "$n" -le 8 ]'
assert "the server still answers" '$BIN list-sessions >/dev/null 2>&1'
sleep 2
n2=$(grep -c . "$T/loop.out" 2>/dev/null || echo 0)
assert "and it has stopped" '[ "$n" = "$n2" ]'
$BIN set-hook -u after-set-option >/dev/null 2>&1

echo "-- clients"
(sleep 2; printf '\002d'; sleep 1) | timeout 30 script -q "$T/screen.out" sh -c "stty rows 24 cols 80; $BIN attach -t 'r'\"'\"'2'" >/dev/null
assert "client-attached fires" 'seen client-attached'
assert "client-detached fires" 'seen client-detached'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
