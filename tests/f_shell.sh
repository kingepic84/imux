#!/bin/bash
# run-shell and if-shell (M35): from the configuration and from keys, the time limit, and the
# two lines that hold: the socket cannot run them, and a name a pane could have set is never
# pasted into shell text (it is refused, quoted with q:, or read from the environment).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-shell.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
T="$TMPDIR"

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

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cat >"$IMUX_CONFIG" <<EOF
set -g automatic-rename off
set -g run-shell-timeout 4
run-shell "echo from-config > $T/cfg.out"
run-shell -b "echo quiet > $T/quiet.out"
if-shell "true" "run-shell 'echo then > $T/if1.out'" "run-shell 'echo else > $T/if1.out'"
if-shell "false" "run-shell 'echo then > $T/if2.out'" "run-shell 'echo else > $T/if2.out'"
if-shell "false" "run-shell 'echo then > $T/if3.out'"
if-shell -F "1" "run-shell 'echo F-yes > $T/if4.out'" "run-shell 'echo F-no > $T/if4.out'"
if-shell -F "#{?0,1,}" "run-shell 'echo F-yes > $T/if5.out'" "run-shell 'echo F-no > $T/if5.out'"
run-shell -C "set -g display-time 1234"
run-shell -b "sleep 31337"
set -g display-panes-time 1
if-shell "true" "set -g display-panes-time 2"
set -g display-panes-time 3
run 'echo ignored > $T/run.out'
bind C run-shell "sleep 1; echo first > $T/order.out" \; run-shell "echo second >> $T/order.out" \; run-shell -b "echo third >> $T/order.out"
bind T run-shell "echo #{window_name} > $T/tainted.out"
bind Q run-shell "echo #{q:window_name} > $T/quoted.out"
bind E run-shell 'printf "%s" "\$IMUX_WINDOW_NAME" > $T/env.out'
bind I run-shell "echo #{pane_id} #{window_index} > $T/ids.out"
bind P run-shell "pwd > $T/pwd.out"
bind K run-shell -C "set -g status-left #{window_name}"
bind L run-shell -C "set -g status-right #{c:window_name}"
EOF

$BIN new-session -d -s s -n "it's a name" >/dev/null 2>&1

assert "the long command is running at first" 'pgrep -f "sleep 31337" >/dev/null'

echo "-- from the configuration"
assert "run-shell in the configuration runs" 'until_true "[ -f $T/cfg.out ]" 40 && [ "$(cat $T/cfg.out)" = from-config ]'
assert "run-shell -b runs too" 'until_true "[ -f $T/quiet.out ]" 40'
assert "if-shell true takes the first command" 'until_true "[ -f $T/if1.out ]" 40 && [ "$(cat $T/if1.out)" = then ]'
assert "if-shell false takes the second" 'until_true "[ -f $T/if2.out ]" 40 && [ "$(cat $T/if2.out)" = else ]'
sleep 1
assert "if-shell false without an else does nothing" '[ ! -f $T/if3.out ]'
assert "if-shell -F tests a format" 'until_true "[ -f $T/if4.out ]" 40 && [ "$(cat $T/if4.out)" = F-yes ]'
assert "a format that is empty is false" 'until_true "[ -f $T/if5.out ]" 40 && [ "$(cat $T/if5.out)" = F-no ]'
assert "run-shell -C runs an imux command" '[ "$($BIN show-options -v display-time)" = 1234 ]'
assert "run runs like run-shell -b" 'until_true "[ -f $T/run.out ]" 40 && [ "$(cat $T/run.out)" = ignored ]'
assert "a line after an if-shell sees its result first (3, not 2)" '[ "$($BIN show-options -v display-panes-time)" = 3 ]'

echo "-- the time limit"
assert "a command over run-shell-timeout is stopped with its group" 'until_true "! pgrep -f \"sleep 31337\" >/dev/null" 40'

echo "-- the socket cannot run shell commands"
$BIN run-shell "echo sock > $T/sock.out" >"$T/o1" 2>&1
sleep 1
assert "run-shell from imux <command> is refused" 'grep -aq "only a configuration" "$T/o1" && [ ! -f $T/sock.out ]'
$BIN if-shell true "run-shell 'echo sock > $T/sock2.out'" >"$T/o2" 2>&1
sleep 1
assert "if-shell from imux <command> is refused" 'grep -aq "only a configuration" "$T/o2" && [ ! -f $T/sock2.out ]'

echo "-- keys: a name from the pane"
run_keys() {
  (sleep 2; for k in "$@"; do printf '\002%s' "$k"; sleep 1.5; done; printf '\002d'; sleep 1) \
    | timeout 60 script -q "$T/screen.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null
}
LEFT_BEFORE=$($BIN show-options -v status-left)
RIGHT_BEFORE=$($BIN show-options -v status-right)
run_keys T Q E I P C K L
assert "a window name in shell text is refused" '[ ! -f $T/tainted.out ] && grep -aq "can be set by a program in the pane" "$T/screen.out"'
assert "#{q:name} gives one quoted word" 'until_true "[ -f $T/quoted.out ]" 20 && [ "$(cat $T/quoted.out)" = "it'"'"'s a name" ]'
assert "\$IMUX_WINDOW_NAME carries the name as it is" '[ "$(cat $T/env.out)" = "it'"'"'s a name" ]'
assert "ids and numbers are fine in the text" '[ "$(cat $T/ids.out | tr -d " ")" != "" ] && grep -Eq "^%?[0-9]+ [0-9]+$" "$T/ids.out"'
assert "the command runs in the directory of the pane" '[ -s $T/pwd.out ]'
assert "a chain on a key waits for each shell command, not for -b" 'until_true "[ \"\$(grep -c . $T/order.out 2>/dev/null)\" = 3 ]" 20 && [ "$(head -2 $T/order.out | tr "\n" " ")" = "first second " ]'

assert "run-shell -C refuses a name a pane could have set" '[ "$($BIN show-options -v status-left)" = "$LEFT_BEFORE" ] && grep -aq "run-shell -C: " "$T/screen.out"'
assert "run-shell -C takes #{c:name} as one quoted word" '[ "$($BIN show-options -v status-right)" != "$RIGHT_BEFORE" ] && $BIN show-options -v status-right | grep -aq "a name"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
