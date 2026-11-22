#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-remote.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 200; $BIN kill-server >/dev/null 2>&1) &

echo "-- looking: list, display-message, has-session"
printf 'set copy-command ""\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=22 client "$TMPDIR/a.out" 'echo up\r' >/dev/null &
CLIENT=$!
sleep 3
$BIN list-sessions >"$TMPDIR/o1" 2>&1
check "list-sessions" '^w: 1 window \(attached\)$' "$TMPDIR/o1"
$BIN list-windows >"$TMPDIR/o2" 2>&1
check "list-windows shows index, name, flags and pane count" '^0: [a-z]+\* \(1 pane\)$' "$TMPDIR/o2"
$BIN list-panes >"$TMPDIR/o3" 2>&1
check "list-panes shows the size, the id and the active one" '^0: \[100x29\] %0 \(active\)$' "$TMPDIR/o3"
$BIN list-panes -F '#{pane_id} #{pane_index} #{window_id} #{session_id}' >"$TMPDIR/o4" 2>&1
check "-F fills in variables" '^%0 0 @[0-9]+ \$0$' "$TMPDIR/o4"
$BIN display-message -p '#{session_name}:#{window_index}.#{pane_index}' >"$TMPDIR/o5" 2>&1
check "display-message -p prints for the session used last" '^w:0\.0$' "$TMPDIR/o5"
$BIN display-message -p -t w:0.0 '#{pane_width}x#{pane_height}' >"$TMPDIR/o6" 2>&1
check "a full target names the pane" '^100x29$' "$TMPDIR/o6"
$BIN display-message -p -t %0 '#{pane_id}' >"$TMPDIR/o7" 2>&1
check "a pane id is a target" '^%0$' "$TMPDIR/o7"
$BIN has-session -t w >/dev/null 2>&1
assert "has-session finds the session" '[ $? -eq 0 ]'
$BIN has-session -t nosuch >"$TMPDIR/o8" 2>&1
assert "and fails for another name" '[ $? -ne 0 ] && grep -aq "can.t find session: nosuch" "$TMPDIR/o8"'
$BIN list-clients >"$TMPDIR/o9" 2>&1
check "list-clients" '^client-[0-9]+: w \[100x30\]$' "$TMPDIR/o9"
$BIN list-panes -t nosuch >"$TMPDIR/o10" 2>&1
assert "a target that does not exist is an error" '[ $? -ne 0 ] && grep -aq "can.t find" "$TMPDIR/o10"'

echo "-- typing and reading: send-keys, capture-pane"
$BIN send-keys -t %0 'echo sent-$((1+2))' Enter >/dev/null 2>&1
sleep 1
$BIN capture-pane -p -t %0 >"$TMPDIR/o11" 2>&1
check "send-keys types text and named keys, capture-pane prints the screen" '^sent-3$' "$TMPDIR/o11"
$BIN capture-pane -p -S 0 -E 0 >"$TMPDIR/o12" 2>&1
assert "a range of one line is one line" '[ "$(wc -l <"$TMPDIR/o12" | tr -d " ")" = 1 ]'
$BIN send-keys -t %0 -l 'echo lit-$((3+3))' >/dev/null 2>&1
$BIN send-keys -t %0 Enter >/dev/null 2>&1
sleep 1
$BIN capture-pane -p >"$TMPDIR/o13" 2>&1
check "-l types key names as text, a key name alone is the key" '^lit-6$' "$TMPDIR/o13"
$BIN send-keys -t %0 -N 3 x >/dev/null 2>&1
sleep 0.5
$BIN capture-pane -p >"$TMPDIR/o14" 2>&1
check "-N repeats" 'xxx' "$TMPDIR/o14"
$BIN send-keys -t %0 C-u >/dev/null 2>&1

echo "-- copy mode from outside"
$BIN copy-mode >/dev/null 2>&1
sleep 0.5
$BIN display-message -p '#{pane_in_mode}' >"$TMPDIR/o15" 2>&1
check "a command of the prompt runs for the attached client" '^1$' "$TMPDIR/o15"
$BIN send-keys -X -t %0 cancel >/dev/null 2>&1
$BIN display-message -p '#{pane_in_mode}' >"$TMPDIR/o16" 2>&1
check "send-keys -X runs a copy mode action" '^0$' "$TMPDIR/o16"

echo "-- naming and selecting"
$BIN rename-window -t w:0 renamed >/dev/null 2>&1
$BIN list-windows -F '#{window_name}' >"$TMPDIR/o17" 2>&1
check "rename-window" '^renamed$' "$TMPDIR/o17"
$BIN rename-session -t w other >/dev/null 2>&1
$BIN list-sessions -F '#{session_name}' >"$TMPDIR/o18" 2>&1
check "rename-session" '^other$' "$TMPDIR/o18"
$BIN has-session -t w >/dev/null 2>&1
assert "the old name is gone" '[ $? -ne 0 ]'
$BIN has-session -t oth >/dev/null 2>&1
assert "a unique prefix of a name finds the session" '[ $? -eq 0 ]'
$BIN display-message -p -t other:renamed.0 '#{window_name}' >"$TMPDIR/o19" 2>&1
check "a window can be named in a target" '^renamed$' "$TMPDIR/o19"

$BIN frobnicate >"$TMPDIR/o24" 2>&1
assert "an unknown command is the usage text and a failure" '[ $? -ne 0 ] && grep -aq "usage: imux" "$TMPDIR/o24"'

echo "-- from inside a pane"
$BIN set -g command-from-panes off >/dev/null 2>&1
$BIN send-keys -t %0 'rm -f "$TMPDIR/inside.txt"; ' >/dev/null 2>&1
$BIN send-keys -t %0 '$BIN send-keys -t %0 x > "$TMPDIR/inside.txt" 2>&1; $BIN list-windows -F "inside-#{window_name}" >> "$TMPDIR/inside.txt" 2>&1; $BIN capture-pane -p -t %0 >> "$TMPDIR/inside.txt" 2>&1' Enter >/dev/null 2>&1
sleep 3
check "typing into panes is refused to a program in a pane" 'not allowed from inside a pane' "$TMPDIR/inside.txt"
check "looking is allowed" 'inside-renamed' "$TMPDIR/inside.txt"
check "and so is reading its own pane" 'inside\.txt' "$TMPDIR/inside.txt"

wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- panes: select-pane, kill-pane, select-window"
ARGS="new -s w" HOLD=14 client "$TMPDIR/c.out" 'echo up\r' '\002%' '\002c' '\002p' >/dev/null &
CLIENT=$!
sleep 6
$BIN list-panes -t w:0 -F '#{pane_id}#{pane_active}' >"$TMPDIR/o20" 2>&1
check "the split made a second pane, which is active" '^%11$' "$TMPDIR/o20"
$BIN select-pane -t w:0.%0 >/dev/null 2>&1
$BIN select-pane -t %0 >/dev/null 2>&1
$BIN list-panes -t w:0 -F '#{pane_id}#{pane_active}' >"$TMPDIR/o21" 2>&1
check "select-pane -t focuses a pane" '^%01$' "$TMPDIR/o21"
$BIN select-pane -t w:0 -R >/dev/null 2>&1
$BIN list-panes -t w:0 -F '#{pane_id}#{pane_active}' >"$TMPDIR/o22" 2>&1
check "select-pane -R moves to the pane on the right" '^%11$' "$TMPDIR/o22"
$BIN list-windows -F '#{window_index}#{window_active}' >"$TMPDIR/o23" 2>&1
check "the client moved to the first window again" '^01$' "$TMPDIR/o23"
$BIN select-window -t w:1 >/dev/null 2>&1
$BIN list-windows -F '#{window_index}#{window_active}' >"$TMPDIR/o24b" 2>&1
check "select-window -t selects" '^11$' "$TMPDIR/o24b"
$BIN select-window -l >/dev/null 2>&1
$BIN list-windows -F '#{window_index}#{window_active}' >"$TMPDIR/o24c" 2>&1
check "select-window -l goes back" '^01$' "$TMPDIR/o24c"
$BIN kill-pane -t %1 >/dev/null 2>&1
sleep 1
$BIN list-panes -t w:0 -F '#{pane_id}' >"$TMPDIR/o25" 2>&1
assert "kill-pane closes it" '[ "$(wc -l <"$TMPDIR/o25" | tr -d " ")" = 1 ]'
$BIN kill-window -t w:1 >/dev/null 2>&1
sleep 1
$BIN list-windows >"$TMPDIR/o26" 2>&1
assert "kill-window closes a window" '[ "$(wc -l <"$TMPDIR/o26" | tr -d " ")" = 1 ]'
wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- command-from-panes on"
printf 'set copy-command ""\nset command-from-panes on\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=1 client "$TMPDIR/b.out" \
  '$BIN send-keys -t %0 "echo viapane-$((2+2))" Enter\r' >/dev/null
check "with the option on a program in a pane can type into panes" 'viapane-4' "$TMPDIR/b.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
