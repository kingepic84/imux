#!/bin/bash
# What the common tmux plugins (tpm, sensible, resurrect, continuum) need from the command line:
# command names given by their unique start (`show-option`), `start-server` through the shim,
# `show-options -t window`, `new-window -t session:N`, a pane title that is never empty, the
# `start_time` variable, and a `display-message` with nobody looking.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qg.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 90; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s pl >/dev/null 2>&1
SHIM=$(ls -d "$(dirname "$($BIN display-message -p '#{socket_path}')")/shims" 2>/dev/null)
export TMUX
TMUX="$($BIN display-message -p '#{socket_path},1,0')"

$BIN set -g @who me >/dev/null 2>&1
assert "show-option (no s) is show-options" '[ "$($BIN show-option -gqv @who)" = me ]'
assert "a longer prefix works too" '[ "$($BIN show-options -gqv @who)" = me ]'
$BIN list >"$TMPDIR/amb" 2>&1
assert "a prefix of several commands is ambiguous" 'grep -aq "ambiguous command: list" "$TMPDIR/amb"'
"$SHIM/tmux" frobnicate >"$TMPDIR/unk" 2>&1
assert "a name that starts nothing is unknown" 'grep -aq "unknown command: frobnicate" "$TMPDIR/unk"'
"$SHIM/tmux" sh >"$TMPDIR/short" 2>&1
assert "a very short prefix is not guessed" 'grep -aq "unknown command: sh" "$TMPDIR/short"'

assert "display-message with nobody attached succeeds" '$BIN display-message "hello" >/dev/null 2>&1'
title=$($BIN display-message -p '#{pane_title}' 2>/dev/null)
assert "a pane nobody titled has a title (the host name)" '[ -n "$title" ]'
start=$($BIN display-message -p '#{start_time}' 2>/dev/null)
now=$(date +%s)
assert "start_time is the server start in epoch seconds" '[ "$start" -le "$now" ] && [ "$start" -gt "$((now - 120))" ]'

assert "show-options -t window reads automatic-rename of that window (on)" '[ "$($BIN show-options -vt pl:0 automatic-rename)" = on ]'
$BIN rename-window -t pl:0 fixed >/dev/null 2>&1
assert "off after the window was named" '[ "$($BIN show-options -vt pl:0 automatic-rename)" = off ]'
$BIN setw -t pl:0 synchronize-panes on >/dev/null 2>&1
assert "a window option reads its own value" '[ "$($BIN show-options -vt pl:0 synchronize-panes)" = on ]'
assert "with no override it reads the global one" '[ "$($BIN show-options -gv synchronize-panes)" = off ]'

$BIN new-window -d -t pl:1 -n two >/dev/null 2>&1
assert "new-window -t session:N makes the window when N is next" '[ "$($BIN list-windows -t pl -F "#{window_name}" | tr "\n" " ")" = "fixed two " ]'
$BIN new-window -d -t pl:1 -n three >"$TMPDIR/inuse" 2>&1
assert "a number in use is refused" 'grep -aq "index 1 in use" "$TMPDIR/inuse"'
assert "and nothing was made" '[ "$($BIN list-windows -t pl | wc -l | tr -d " ")" = 2 ]'

assert "the shim directory exists" '[ -x "$SHIM/tmux" ]'
"$SHIM/tmux" start-server >"$TMPDIR/ss" 2>&1
assert "tmux start-server through the shim does nothing and succeeds" '[ "$?" = 0 ] && [ ! -s "$TMPDIR/ss" ]'
out=$("$SHIM/tmux" 'start-server;' show-options -gqv @who 2>&1)
assert "the form tpm uses (start-server with a glued semicolon) then a command" '[ "$out" = me ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
