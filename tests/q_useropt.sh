#!/bin/bash
# User options (`@name`): set, append, unset, scope-free reads in formats (display-message, list-*,
# the status bar), show-options, limits, names, and the taint of a value a pane gave when it is
# used in shell text.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qu.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
ESC=$(printf '\033')

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g @from_config cfg-value\nset -gq @quiet_one q1\nset -g status-left "L:#{@from_config}:"\nset -g status-right ""\nset -g command-from-panes on\n' >"$IMUX_CONFIG"

$BIN new-session -d -s s -n one >/dev/null 2>&1

echo "-- values from the configuration"
assert "a configuration value reads back in display-message" '[ "$($BIN display-message -p "#{@from_config}")" = cfg-value ]'
assert "set -gq on a user option stores it" '[ "$($BIN display-message -p "#{@quiet_one}")" = q1 ]'
assert "an unset user option is empty" '[ -z "$($BIN display-message -p "#{@nope}")" ]'
cond=$($BIN display-message -p '#{?@from_config,yes,no}' 2>&1)
assert "a conditional sees it" '[ "$cond" = yes ]'

echo "-- set, append, unset through the socket"
$BIN set -g @a one >/dev/null 2>&1
assert "set stores" '[ "$($BIN display-message -p "#{@a}")" = one ]'
$BIN set -ag @a -two >/dev/null 2>&1
assert "set -a appends" '[ "$($BIN display-message -p "#{@a}")" = one-two ]'
$BIN set -g @a "with space" >/dev/null 2>&1
assert "a value with a space" '[ "$($BIN display-message -p "#{@a}")" = "with space" ]'
$BIN set -gu @a >/dev/null 2>&1
assert "set -u removes it" '[ -z "$($BIN display-message -p "#{@a}")" ]'
$BIN setw -g @w winval >/dev/null 2>&1
assert "setw -g stores a user option too" '[ "$($BIN display-message -p "#{@w}")" = winval ]'

echo "-- show-options"
$BIN show-options -gv @from_config >"$TMPDIR/v.out" 2>&1
assert "show-options -v prints the value" 'grep -aqx "cfg-value" "$TMPDIR/v.out"'
$BIN show-options -g @from_config >"$TMPDIR/s.out" 2>&1
assert "show-options prints name and value" 'grep -aqx "@from_config cfg-value" "$TMPDIR/s.out"'
$BIN show-options >"$TMPDIR/all.out" 2>&1
assert "the full listing has the user options too" 'grep -aqx "@from_config cfg-value" "$TMPDIR/all.out" && grep -aq "^status-left " "$TMPDIR/all.out"'
$BIN show-options -gqv @missing >"$TMPDIR/q.out" 2>&1
assert "-q on a missing user option is quiet and empty" '[ $? = 0 ] && [ ! -s "$TMPDIR/q.out" ]'
$BIN show-options -gv @missing >"$TMPDIR/m.out" 2>&1
assert "a missing user option without -q is an error" '[ $? != 0 ] || grep -aqi "unknown" "$TMPDIR/m.out"'

echo "-- formats elsewhere"
assert "list-windows -F reads it" '[ "$($BIN list-windows -t s -F "#{window_name}:#{@from_config}")" = "one:cfg-value" ]'
assert "list-panes -F reads it" '[ "$($BIN list-panes -t s -F "#{@from_config}")" = cfg-value ]'
assert "list-clients is not needed: a menu title reads it" '$BIN display-menu -t s:one -T "#{@from_config}" x x "display-message ok" </dev/null >/dev/null 2>&1; true'

echo "-- the status bar"
(sleep 3; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/c1.out" sh -c "stty rows 24 cols 80; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
wait $CL
assert "status-left shows the configuration value" 'grep -aq "L:cfg-value:" "$TMPDIR/c1.out"'
$BIN set -g @from_config changed >/dev/null 2>&1
(sleep 3; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/c2.out" sh -c "stty rows 24 cols 80; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
wait $CL
assert "a change shows on the next draw" 'grep -aq "L:changed:" "$TMPDIR/c2.out"'

echo "-- names and limits"
$BIN set -g "@bad name" x >"$TMPDIR/n1.out" 2>&1
assert "a name with a space is refused" 'grep -aqi "bad user option name" "$TMPDIR/n1.out"'
$BIN set -g @ x >"$TMPDIR/n2.out" 2>&1
assert "a bare @ is refused" 'grep -aqi "bad user option name" "$TMPDIR/n2.out"'
big=$(head -c 5000 /dev/zero | tr '\0' x)
$BIN set -g @big "$big" >"$TMPDIR/n3.out" 2>&1
assert "a value over 4 KiB is refused" 'grep -aqi "longer than 4096" "$TMPDIR/n3.out"'
$BIN set -g @ctl "$(printf 'a\033[31mb')" >/dev/null 2>&1
assert "control characters are dropped from a value" '[ "$($BIN display-message -p "#{@ctl}")" = "a[31mb" ]'

echo "-- shell text and the taint"
$BIN set -g @mine trusted >/dev/null 2>&1
$BIN send-keys -t s:one "$BIN set -g @theirs from-a-pane" Enter >/dev/null 2>&1
sleep 2
assert "a pane may set one when command-from-panes is on" '[ "$($BIN display-message -p "#{@theirs}")" = from-a-pane ]'
$BIN bind x run-shell "echo #{@mine} >$TMPDIR/rs1.out" >/dev/null 2>&1
$BIN bind y run-shell "echo #{@theirs} >$TMPDIR/rs2.out" >/dev/null 2>&1
$BIN bind z run-shell "echo #{q:@theirs} >$TMPDIR/rs3.out" >/dev/null 2>&1
(sleep 3; printf '\002x'; sleep 1; printf '\002y'; sleep 1; printf '\002z'; sleep 1; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/c3.out" sh -c "stty rows 24 cols 120; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
wait $CL
assert "a value the user set may be used in shell text" 'grep -aqx trusted "$TMPDIR/rs1.out"'
assert "a value a pane set is refused unquoted, naming the way out" '[ ! -s "$TMPDIR/rs2.out" ] && grep -aq "can be set by a program in the pane" "$TMPDIR/c3.out"'
assert "written #{q:...} it is allowed" 'grep -aqx from-a-pane "$TMPDIR/rs3.out"'
$BIN set -g @theirs mine-now >/dev/null 2>&1
(sleep 3; printf '\002y'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/c4.out" sh -c "stty rows 24 cols 120; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
wait $CL
assert "the user setting it again makes it trusted" 'grep -aqx mine-now "$TMPDIR/rs2.out"'

echo "-- a dry read keeps nothing and starts nothing"
$BIN kill-server >/dev/null 2>&1
sleep 1
$BIN check-config "$IMUX_CONFIG" >"$TMPDIR/cc.out" 2>&1
assert "check-config still lists them as information" 'grep -aq "user-option: @from_config" "$TMPDIR/cc.out"'

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
