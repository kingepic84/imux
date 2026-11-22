#!/bin/bash
# Window-scoped options (setw), synchronize-panes in particular: what is typed into one pane goes
# to every live pane of the window; a window without a value of its own follows the global option.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qw.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

shows() {
  local pane=$1 pattern=$2 i
  for i in $(seq 1 40); do
    $BIN capture-pane -p -t "$pane" 2>/dev/null | grep -aqE -- "$pattern" && return 0
    sleep 0.25
  done
  return 1
}

var() {
  $BIN display-message -p -t "$1" "$2" 2>&1
}

$BIN new-session -d -s a -n one >/dev/null 2>&1
A1=$($BIN list-panes -t a:one -F '#{pane_id}' | head -1)
A2=$($BIN split-window -t a:one -d -P -F '#{pane_id}')
$BIN new-window -t a -d -n two >/dev/null 2>&1
B1=$($BIN list-panes -t a:two -F '#{pane_id}' | head -1)
B2=$($BIN split-window -t a:two -d -P -F '#{pane_id}')
sleep 1

echo "-- off by default"
assert "a pane is not synchronized" '[ "$(var "$A1" "#{pane_synchronized}")" = 0 ]'
$BIN send-keys -t "$A1" 'echo first-$((1+1))' Enter
assert "typing reaches the pane it is sent to" 'shows "$A1" "^first-2$"'
sleep 0.5
$BIN capture-pane -p -t "$A2" >"$TMPDIR/c1"
assert "and no other" '! grep -aq "^first-2$" "$TMPDIR/c1"'

echo "-- setw on a window"
$BIN setw -t a:one synchronize-panes on >"$TMPDIR/o1" 2>&1
assert "setw -t succeeds" '[ $? -eq 0 ]'
assert "the pane says so" '[ "$(var "$A1" "#{pane_synchronized}")" = 1 ] && [ "$(var "$A2" "#{window_synchronized}")" = 1 ]'
$BIN send-keys -t "$A1" 'echo second-$((1+2))' Enter
assert "send-keys reaches the pane it names" 'shows "$A1" "^second-3$"'
assert "and the other pane of the window" 'shows "$A2" "^second-3$"'
$BIN send-keys -t "$B1" 'echo other-$((2+2))' Enter
assert "another window is not synchronized" 'shows "$B1" "^other-4$"'
sleep 0.5
$BIN capture-pane -p -t "$B2" >"$TMPDIR/c2"
assert "its other pane gets nothing" '! grep -aq "^other-4$" "$TMPDIR/c2"'
$BIN capture-pane -p -t "$A2" >"$TMPDIR/c3"
assert "and nothing from it reached the first window" '! grep -aq "^other-4$" "$TMPDIR/c3"'

echo "-- toggle and unset"
$BIN setw -t a:one synchronize-panes >/dev/null 2>&1
assert "no value flips it off" '[ "$(var "$A1" "#{pane_synchronized}")" = 0 ]'
$BIN send-keys -t "$A1" 'echo third-$((3+3))' Enter
assert "off again, only the named pane gets the keys" 'shows "$A1" "^third-6$"'
sleep 0.5
$BIN capture-pane -p -t "$A2" >"$TMPDIR/c4"
assert "the other pane did not" '! grep -aq "^third-6$" "$TMPDIR/c4"'
$BIN setw -t a:one synchronize-panes >/dev/null 2>&1
assert "no value flips it on" '[ "$(var "$A1" "#{pane_synchronized}")" = 1 ]'
$BIN setw -t a:one -u synchronize-panes >/dev/null 2>&1
assert "-u gives the window the global option again" '[ "$(var "$A1" "#{pane_synchronized}")" = 0 ]'

echo "-- the global option and a window's own value"
$BIN set -g synchronize-panes on >/dev/null 2>&1
assert "set -g turns it on for every window without a value" '[ "$(var "$A1" "#{pane_synchronized}")" = 1 ] && [ "$(var "$B1" "#{pane_synchronized}")" = 1 ]'
$BIN setw -t a:two synchronize-panes off >/dev/null 2>&1
assert "a window's own off beats the global on" '[ "$(var "$B1" "#{pane_synchronized}")" = 0 ] && [ "$(var "$A1" "#{pane_synchronized}")" = 1 ]'
$BIN setw -g synchronize-panes off >/dev/null 2>&1
assert "setw -g is the global option" '[ "$(var "$A1" "#{pane_synchronized}")" = 0 ]'
$BIN show-options synchronize-panes >"$TMPDIR/o2" 2>&1
check "and show-options shows it" '^synchronize-panes off$' "$TMPDIR/o2"

echo "-- mistakes"
$BIN setw -t a:one synchronize-panes maybe >"$TMPDIR/o3" 2>&1
assert "a bad value is refused" '[ $? -ne 0 ] && grep -aq "expected on or off" "$TMPDIR/o3"'
$BIN setw -t nosuch synchronize-panes on >"$TMPDIR/o4" 2>&1
assert "a missing window is an error" '[ $? -ne 0 ]'

echo "-- a pane that is dead is skipped"
$BIN set -g remain-on-exit on >/dev/null 2>&1
$BIN setw -t a:one synchronize-panes off >/dev/null 2>&1
$BIN send-keys -t "$A2" 'exit' Enter
sleep 1
$BIN setw -t a:one synchronize-panes on >/dev/null 2>&1
$BIN send-keys -t "$A1" 'echo fifth-$((5+5))' Enter
assert "a dead pane does not stop the others" 'shows "$A1" "^fifth-10$"'
assert "and send-keys to the live pane succeeds" '$BIN send-keys -t "$A1" "" >/dev/null 2>&1'

echo "-- typed through a client"
$BIN setw -t a:one synchronize-panes on >/dev/null 2>&1
$BIN respawn-pane -k -t "$A2" >/dev/null 2>&1
sleep 1
(sleep 1.5; printf 'echo typed-$((6+6))\r'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/client.out" sh -c "stty rows 30 cols 100; $BIN attach -t a" >/dev/null 2>&1
assert "text typed at a client reaches the active pane" 'shows "$A1" "^typed-12$"'
assert "and the other pane of a synchronized window" 'shows "$A2" "^typed-12$"'

echo "-- a pane in copy mode is skipped"
$BIN select-pane -t "$A1" >/dev/null 2>&1
(sleep 1.5; printf '\002o'; sleep 0.7; printf '\002['; sleep 0.7; printf '\002o'; sleep 0.7; printf 'echo copyskip-$((7+7))\r'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/client2.out" sh -c "stty rows 30 cols 100; $BIN attach -t a" >/dev/null 2>&1
assert "the active pane gets what is typed" 'shows "$A1" "^copyskip-14$"'
$BIN capture-pane -p -t "$A2" >"$TMPDIR/c6"
assert "the pane in copy mode does not" '! grep -aq "^copyskip-14$" "$TMPDIR/c6"'

echo "-- keys can be bound"
$BIN bind-key X setw synchronize-panes >"$TMPDIR/o5" 2>&1
assert "bind-key accepts setw" '[ $? -eq 0 ]'
$BIN list-keys >"$TMPDIR/o6" 2>&1
check "list-keys shows it" 'bind-key -T prefix X +set-window-option synchronize-panes' "$TMPDIR/o6"
$BIN bind-key Y wait-for ready >"$TMPDIR/o7" 2>&1
assert "another option cannot be bound" '[ $? -ne 0 ]'

echo "-- activity and bell flags"
EVA="$TMPDIR/alerts.out"
flags() { $BIN list-windows -t m -F '#{window_name}=#{window_flags}' 2>&1; }
until_flags() {
  local i
  for i in $(seq 1 40); do
    flags | tr '\n' ' ' | grep -aqE -- "$1" && return 0
    sleep 0.25
  done
  return 1
}
$BIN new-session -d -s m -n cur >/dev/null 2>&1
$BIN new-window -t m -d -n bg >/dev/null 2>&1
sleep 1
$BIN send-keys -t m:bg 'echo quiet-$((1+1))' Enter
assert "monitors are off for activity by default" 'shows m:bg "^quiet-2$" && flags | grep -aqx "bg="'
$BIN set-hook -g alert-bell "run-shell \"echo bell:#{q:hook_window_name} >> $EVA\"" >/dev/null 2>&1
$BIN set-hook -g alert-activity "run-shell \"echo activity:#{q:hook_window_name} >> $EVA\"" >/dev/null 2>&1
$BIN set -g monitor-activity on >/dev/null 2>&1
$BIN send-keys -t m:bg 'echo loud-$((2+2))' Enter
assert "output in a background window flags it" 'until_flags "bg=# "'
assert "the variable says so" '[ "$(var m:bg "#{window_activity_flag}#{window_bell_flag}")" = 10 ]'
$BIN send-keys -t m:cur 'echo here-$((3+3))' Enter
assert "with nobody attached the current window is not viewed either" 'until_flags "cur=#\*"'
assert "the hook fires with the window name" 'for i in $(seq 1 40); do grep -aqx "activity:bg" "$EVA" 2>/dev/null && break; sleep 0.25; done; grep -aqx "activity:bg" "$EVA"'
$BIN send-keys -t m:bg "printf '\\007'" Enter
assert "a bell flags the window too" 'until_flags "bg=#! "'
assert "the bell variable" '[ "$(var m:bg "#{window_bell_flag}")" = 1 ]'
assert "alert-bell fires" 'for i in $(seq 1 40); do grep -aqx "bell:bg" "$EVA" 2>/dev/null && break; sleep 0.25; done; grep -aqx "bell:bg" "$EVA"'
$BIN setw -t m:bg monitor-activity off >/dev/null 2>&1
assert "switching a monitor off takes its flag away" 'until_flags "bg=! "'
$BIN setw -t m:bg monitor-activity on >/dev/null 2>&1
$BIN send-keys -t m:bg 'echo again-$((4+4))' Enter
assert "and on flags again" 'until_flags "bg=#! "'
$BIN setw -t m:bg monitor-bell off >/dev/null 2>&1
assert "the bell monitor is per window as well" 'until_flags "bg=# "'
$BIN setw -t m:bg monitor-bell on >/dev/null 2>&1
$BIN send-keys -t m:bg "printf '\\007'" Enter
assert "bell on again" 'until_flags "bg=#! "'

echo "-- a client viewing a window clears it; M-n goes to the next alert"
$BIN list-keys >"$TMPDIR/o8" 2>&1
check "list-keys shows the alert keys" 'bind-key -T prefix M-n +next-window -a$' "$TMPDIR/o8"
(sleep 3; printf '\002\033n'; sleep 4; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/client3.out" sh -c "stty rows 30 cols 100; $BIN attach -t m" >/dev/null 2>&1 &
CL=$!
assert "a viewed window loses its flag" 'until_flags "cur=\* "'
assert "while another keeps it" 'flags | grep -aq "^bg=#!$"'
assert "M-n selects the window with the alert and clears it" 'until_flags "bg=\*"'
assert "it is the current window now" '$BIN list-windows -t m -F "#{window_name}=#{window_active}" | grep -aqx "bg=1"'
wait $CL

echo "-- visual-bell and bell-action"
$BIN set -g visual-bell on >/dev/null 2>&1
$BIN set -g bell-action current >/dev/null 2>&1
(sleep 5; printf '\002d'; sleep 0.5) | timeout 30 script -q "$TMPDIR/client4.out" sh -c "stty rows 30 cols 100; $BIN attach -t m" >/dev/null 2>&1 &
CL=$!
sleep 2
$BIN send-keys -t m:cur "printf '\\007'" Enter
wait $CL
assert "bell-action current: a bell elsewhere shows nothing" '! grep -aq "Bell in window" "$TMPDIR/client4.out"'
$BIN set -g bell-action any >/dev/null 2>&1
(sleep 5; printf '\002d'; sleep 0.5) | timeout 30 script -q "$TMPDIR/client5.out" sh -c "stty rows 30 cols 100; $BIN attach -t m" >/dev/null 2>&1 &
CL=$!
sleep 2
$BIN send-keys -t m:cur "printf '\\007'" Enter
sleep 1.5
wait $CL
assert "visual-bell shows a message with the window number" 'grep -aq "Bell in window 0" "$TMPDIR/client5.out"'

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
