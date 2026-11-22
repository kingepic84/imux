#!/bin/bash
# Format loops over panes, windows and sessions (#{P:fmt[,current]}, #{W:...}, #{S:...}) and the
# search of a pane's screen (#{C:text}, #{C/r:...}, #{C/i:...}) in formats of the socket commands.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-ql.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 60; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s s -n one >/dev/null 2>&1
$BIN new-window -t s -n two >/dev/null 2>&1
$BIN new-window -t s -n three >/dev/null 2>&1
$BIN new-session -d -s t -n other >/dev/null 2>&1
$BIN select-window -t s:one >/dev/null 2>&1
$BIN split-window -d -t s:one >/dev/null 2>&1
$BIN send-keys -t s:one 'echo LOOPMARK-ZZZ' Enter >/dev/null 2>&1
sleep 1.5

fm() {
  $BIN display-message -p -t s:one "$1" 2>&1
}

echo "-- loops"
assert "P: goes over the panes of the window" '[ "$(fm "#{P:x}")" = "xx" ]'
assert "P: sees each pane's own variables" '[ "$(fm "#{P:#{pane_index}.}")" = "0.1." ]'
v=$(fm '#{P:[#{pane_index}],<#{pane_index}>}')
assert "P: has a second format for the active pane" '[ "$v" = "<0>[1]" ]'
v=$(fm '#{W:#{window_name} }')
assert "W: goes over the windows of the session" '[ "$v" = "one two three " ]'
v=$(fm '#{W:#{window_name},*#{window_name}}')
assert "W: has a second format for the current window" '[ "$v" = "*onetwothree" ]'
assert "S: goes over the sessions" '[ "$(fm "#{S:#{session_name} }")" = "s t " ]'
v=$(fm '#{S:#{session_name},<#{session_name}>}')
assert "S: has a second format for the session of the pane" '[ "$v" = "<s>t" ]'
v=$(fm '#{P:#{?loop_first_flag,F,}#{?loop_last_flag,L,}}')
assert "the loop flags are set" '[ "$v" = "FL" ]'
assert "a loop does not nest" '[ "$(fm "#{W:#{S:x}}")" = "#{S:x}#{S:x}#{S:x}" ]'
assert "a name the loop lacks comes from outside" '[ "$(fm "#{P:#{session_name}}")" = "ss" ]'

echo "-- search"
n=$(fm '#{C:LOOPMARK-ZZZ}')
assert "C: gives the row of the first line with the text" '[ "$n" -ge 1 ] 2>/dev/null'
assert "C: gives 0 when nothing holds the text" '[ "$(fm "#{C:nothing-like-this}")" = 0 ]'
assert "C: is case sensitive" '[ "$(fm "#{C:loopmark-zzz}")" = 0 ]'
assert "C/i: ignores case" '[ "$(fm "#{C/i:loopmark-zzz}")" = "$n" ]'
v=$(fm '#{C/r:LOOPMARK-ZZ+$}')
assert "C/r: takes a regular expression" '[ "$v" -ge 1 ] 2>/dev/null'
assert "a pattern that is not a regular expression finds nothing" '[ "$(fm "#{C/r:[}")" = 0 ]'
v=$(fm '#{?#{C:LOOPMARK-ZZZ},found,missing}')
assert "C: works as a condition" '[ "$v" = found ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
