#!/bin/bash
# Format conditionals and comparisons (M34): #{?cond,a,b}, ==, !=, <, >, &&, ||, !, escapes,
# through display-message -p, list-windows -F and the status bar of a config.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-format.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

is() {
  local got
  got=$($BIN display-message -p -t f: "$2" 2>&1)
  if [ "$got" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1 (got: $got, want: $3)"; fail=1; fi
}

(sleep 90; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s f -n alpha >/dev/null 2>&1
$BIN new-window -t f -n beta >/dev/null 2>&1

echo "-- conditionals"
is "true condition picks then" '#{?window_active,yes,no}' 'yes'
is "a word that is no variable counts as text, so true" '#{?pane_title_nothing_here,yes,no}' 'yes'
is "a zero is false" '#{?#{==:1,2},yes,no}' 'no'
is "a variable that is 0 is false" '#{?session_attached,attached,detached}' 'detached'
is "else may be empty" '#{?#{==:1,2},yes,}' ''
is "no else gives nothing" '#{?#{==:1,2},yes}' ''
is "pairs chain" '#{?#{==:1,2},a,#{==:1,1},b,c}' 'b'
is "branches expand" '#{?window_active,[#{window_name}],-}' '[beta]'

echo "-- comparisons and logic"
is "== equal" '#{==:#{window_name},beta}' '1'
is "== not equal" '#{==:#{window_name},alpha}' '0'
is "!= " '#{!=:#{window_name},alpha}' '1'
is "text order <" '#{<:a,b}' '1'
is "text order >" '#{>:a,b}' '0'
is "&& with three operands" '#{&&:1,1,1}' '1'
is "&& with a zero" '#{&&:1,0,1}' '0'
is "|| " '#{||:0,,1}' '1'
is "! " '#{!:0}' '1'
is "nested" '#{?#{&&:#{==:#{window_name},beta},#{!:0}},both,not}' 'both'

echo "-- escapes and malformed input"
is "an escaped comma stays in the operand" '#{?1,a#,b,c}' 'a,b'
is "an escaped brace" '#{?1,{#},c}' '{}'
is "a hash stays a hash" '#{?1,##,c}' '#'
is "malformed ? is left as written" '#{?1}' '#{?1}'
is "malformed == is left as written" '#{==:a}' '#{==:a}'

echo "-- list-windows -F"
$BIN list-windows -t f -F '#{?window_active,*,-}#{window_name}' >"$TMPDIR/lw" 2>&1
assert "each window is formatted by its own state" 'grep -qx -- "-alpha" "$TMPDIR/lw" && grep -qx -- "\*beta" "$TMPDIR/lw"'

echo "-- the status bar"
$BIN kill-server >/dev/null 2>&1
sleep 1
printf '%s\n' 'set -g status-right "#{?#{==:#{session_name},f},IN-F,OUT}"' 'set -g window-status-format "#{?window_active,[#W],#W}"' 'set -g window-status-current-format "#{?window_active,[#W],#W}"' >"$TMPDIR/status.conf"
(sleep 2; printf '\002d'; sleep 1) | IMUX_CONFIG="$TMPDIR/status.conf" timeout 30 script -q "$TMPDIR/screen.out" sh -c "stty rows 24 cols 80; $BIN new -s f" >/dev/null
assert "status-right takes a comparison" 'grep -aq "IN-F" "$TMPDIR/screen.out"'
assert "the current window shows its brackets" 'grep -aq "\[" "$TMPDIR/screen.out"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
