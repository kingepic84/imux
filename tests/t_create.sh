#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-create.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 90; $BIN kill-server >/dev/null 2>&1) &

echo "-- new-session -d starts the server and needs no terminal"
$BIN new-session -d -s a -n win1 >"$TMPDIR/o1" 2>&1
assert "new-session -d succeeds with no server running" '[ $? -eq 0 ]'
$BIN list-sessions >"$TMPDIR/o2" 2>&1
check "the session exists" '^a: 1 window$' "$TMPDIR/o2"
$BIN list-windows -t a -F '#{window_name}' >"$TMPDIR/o3" 2>&1
check "-n names the window" '^win1$' "$TMPDIR/o3"
$BIN new-session -d -s a >"$TMPDIR/o4" 2>&1
assert "a second session of the same name is refused" '[ $? -ne 0 ] && grep -aq "duplicate session: a" "$TMPDIR/o4"'
$BIN new-session -s z >"$TMPDIR/o5" 2>&1
assert "without -d it says how to attach" '[ $? -ne 0 ] && grep -aq -- "-d" "$TMPDIR/o5"'
$BIN new-session -d -s c 'echo hi-$((1+2)); sleep 20' >/dev/null 2>&1
sleep 1
$BIN capture-pane -p -t c: >"$TMPDIR/o6" 2>&1
check "a command is run instead of a shell prompt" '^hi-3$' "$TMPDIR/o6"
$BIN new-session -d -s p -P -F '#{session_name}:#{window_index}' >"$TMPDIR/o7" 2>&1
check "-P prints the new session with -F" '^p:0$' "$TMPDIR/o7"
$BIN new-session -d -s q -c /nonexistent-dir >"$TMPDIR/o8" 2>&1
assert "-c with a missing directory is an error" '[ $? -ne 0 ] && grep -aq "not a directory" "$TMPDIR/o8"'

echo "-- new-window"
$BIN new-window -t a -n second -P -F '#{window_name}' >"$TMPDIR/o9" 2>&1
check "new-window -n -P prints the name" '^second$' "$TMPDIR/o9"
$BIN new-window -t a -d -n third >/dev/null 2>&1
$BIN list-windows -t a -F '#{window_index}#{window_name}#{window_active}' >"$TMPDIR/o10" 2>&1
check "a new window is selected" '^1second1$' "$TMPDIR/o10"
check "with -d it is not" '^2third0$' "$TMPDIR/o10"
$BIN new-window -t a -n multi sh -c 'echo multi-$((2+2)); sleep 20' >/dev/null 2>&1
sleep 1
$BIN capture-pane -p -t a:multi >"$TMPDIR/o11" 2>&1
check "several words are quoted for the shell" '^multi-4$' "$TMPDIR/o11"
$BIN new-window -t nosuch >"$TMPDIR/o12" 2>&1
assert "a missing session is an error" '[ $? -ne 0 ]'

echo "-- split-window"
$BIN split-window -t a:0 -h -P -F '#{pane_id}' >"$TMPDIR/o13" 2>&1
check "split-window prints the new pane" '^%[0-9]+$' "$TMPDIR/o13"
$BIN list-panes -t a:0 -F '#{pane_id}#{pane_active}' >"$TMPDIR/o14" 2>&1
assert "the window has two panes" '[ "$(wc -l <"$TMPDIR/o14" | tr -d " ")" = 2 ]'
check "the new pane is the active one" '^%[0-9]+1$' "$TMPDIR/o14"
$BIN split-window -t a:0 -d -P -F '#{pane_id}' >"$TMPDIR/o15" 2>&1
$BIN list-panes -t a:0 -F '#{pane_active}#{pane_id}' >"$TMPDIR/o16" 2>&1
assert "with -d the focus stays" '[ "$(grep -c "^1" "$TMPDIR/o16")" = 1 ] && [ "$(wc -l <"$TMPDIR/o16" | tr -d " ")" = 3 ]'
NEW=$(tr -d '\n' <"$TMPDIR/o15")
$BIN split-window -t a:0 -c /tmp -P -F '#{pane_id}' >"$TMPDIR/o17" 2>&1
sleep 1
$BIN display-message -p -t "$(tr -d '\n' <"$TMPDIR/o17")" '#{pane_current_path}' >"$TMPDIR/o18" 2>&1
check "-c sets the directory" 'tmp$' "$TMPDIR/o18"
$BIN split-window -t a:0 -l ten >"$TMPDIR/o19" 2>&1
assert "a bad size is refused, not ignored" '[ $? -ne 0 ] && grep -aq "bad size" "$TMPDIR/o19"'
$BIN kill-pane -t "$NEW" >/dev/null 2>&1

echo "-- a program in a pane cannot create things"
$BIN set -g command-from-panes off >/dev/null 2>&1
$BIN new-window -t a -n spawned sh -c 'sleep 1' >/dev/null 2>&1
$BIN send-keys -t a:win1 '$BIN new-window -t a -n evil > "$TMPDIR/evil.txt" 2>&1' Enter >/dev/null 2>&1
for _ in $(seq 1 40); do [ -s "$TMPDIR/evil.txt" ] && break; sleep 0.5; done
check "new-window from inside a pane is refused" 'not allowed from inside a pane' "$TMPDIR/evil.txt"
$BIN list-windows -t a -F '#{window_name}' >"$TMPDIR/o20" 2>&1
assert "and no window was made" '! grep -aq "^evil$" "$TMPDIR/o20"'

echo "-- options and bindings from the shell"
$BIN set-option -g clock-mode-style 12 >/dev/null 2>&1
assert "set-option works" '[ $? -eq 0 ]'
$BIN set -g no-such-option 1 >"$TMPDIR/o21" 2>&1
assert "an unknown option is an error" '[ $? -ne 0 ] && grep -aq "unknown option" "$TMPDIR/o21"'
$BIN bind-key X new-window >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/o22" 2>&1
check "bind-key puts the key in the table" '^bind-key -T prefix X +new-window$' "$TMPDIR/o22"
$BIN unbind-key X >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/o23" 2>&1
assert "unbind-key takes it out" '! grep -aq "^bind-key -T prefix X " "$TMPDIR/o23"'

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
