#!/bin/bash
# The tmux-compatible commands and flags that were gaps: set -q/-o, directional targets, =exact,
# split-window -l/-p, respawn-pane, join-pane, wait-for, pipe-pane, command-alias, capture-pane
# -e/-N, list-keys -T, kill-pane/kill-window -a and show-options.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-compat.XXXXXX)
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

until_true() {
  local n=0
  while [ "$n" -lt "$2" ]; do
    eval "$1" && return 0
    sleep 0.25
    n=$((n + 1))
  done
  return 1
}

panes_of() { $BIN list-panes -t "$1" -F '#{pane_id}' 2>/dev/null | wc -l | tr -d ' '; }
windows_of() { $BIN list-windows -t "$1" 2>/dev/null | wc -l | tr -d ' '; }

(sleep 240; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s base >/dev/null 2>&1

echo "-- set -q, -o and the scope flags; show-options"
$BIN set -g mouse off >/dev/null 2>&1
$BIN show-options -v mouse >"$TMPDIR/o1" 2>&1
check "show-options -v prints the value" '^off$' "$TMPDIR/o1"
$BIN set -o mouse on >/dev/null 2>&1
$BIN show-options -v mouse >"$TMPDIR/o2" 2>&1
check "set -o leaves an option that was set" '^off$' "$TMPDIR/o2"
$BIN set -o escape-time 20 >/dev/null 2>&1
$BIN show-options -v escape-time >"$TMPDIR/o3" 2>&1
check "set -o sets an option nobody set" '^20$' "$TMPDIR/o3"
$BIN set -q no-such-option 1 >/dev/null 2>&1
assert "set -q ignores an unknown option" '[ $? -eq 0 ]'
$BIN set no-such-option 1 >"$TMPDIR/o4" 2>&1
assert "without -q it is an error" '[ $? -ne 0 ] && grep -aq "unknown option" "$TMPDIR/o4"'
$BIN set -w mouse on >/dev/null 2>&1
$BIN set -s escape-time 15 >/dev/null 2>&1
$BIN show-options -v mouse >"$TMPDIR/o5" 2>&1
check "-w is accepted" '^on$' "$TMPDIR/o5"
$BIN show-options >"$TMPDIR/o6" 2>&1
check "show-options lists every option" '^history-limit 50000$' "$TMPDIR/o6"
check "with quoting where a value needs it" '^status-right ".*"$' "$TMPDIR/o6"
$BIN show-options nothing-here >"$TMPDIR/o7" 2>&1
assert "an unknown option is an error" '[ $? -ne 0 ]'
$BIN show-options -q nothing-here >"$TMPDIR/o7" 2>&1
assert "-q makes it silent" '[ $? -eq 0 ] && [ ! -s "$TMPDIR/o7" ]'

echo "-- directional, edge and exact targets"
$BIN new-session -d -s work -n one >/dev/null 2>&1
$BIN new-session -d -s work2 >/dev/null 2>&1
$BIN has-session -t =work >/dev/null 2>&1
assert "=name matches exactly" '[ $? -eq 0 ]'
$BIN has-session -t =wor >/dev/null 2>&1
assert "=name does not match a prefix" '[ $? -ne 0 ]'
$BIN split-window -h -t work:0 >/dev/null 2>&1
$BIN list-panes -t work:0 -F '#{pane_id}' >"$TMPDIR/o8" 2>&1
LEFT=$(sed -n 1p "$TMPDIR/o8")
RIGHT=$(sed -n 2p "$TMPDIR/o8")
$BIN display-message -p -t 'work:0.{left}' '#{pane_id}' >"$TMPDIR/o9" 2>&1
check "{left} is the leftmost pane" "^$LEFT\$" "$TMPDIR/o9"
$BIN display-message -p -t 'work:0.{right}' '#{pane_id}' >"$TMPDIR/o10" 2>&1
check "{right} is the rightmost pane" "^$RIGHT\$" "$TMPDIR/o10"
$BIN display-message -p -t 'work:0.{left-of}' '#{pane_id}' >"$TMPDIR/o11" 2>&1
check "{left-of} is beside the active pane" "^$LEFT\$" "$TMPDIR/o11"
$BIN display-message -p -t 'work:0.{right-of}' '#{pane_id}' >"$TMPDIR/o12" 2>&1
assert "{right-of} with nothing there is an error" '[ $? -ne 0 ] && grep -aq "can.t find pane" "$TMPDIR/o12"'
$BIN display-message -p -t 'work:0.{top-right}' '#{pane_id}' >"$TMPDIR/o13" 2>&1
check "a corner names the pane that covers it" "^$RIGHT\$" "$TMPDIR/o13"

echo "-- split-window -l and -p"
$BIN new-session -d -s sz >/dev/null 2>&1
$BIN split-window -h -l 20 -t sz:0 -P -F '#{pane_width}' >"$TMPDIR/o14" 2>&1
check "-l is cells" '^20$' "$TMPDIR/o14"
$BIN new-session -d -s sz2 >/dev/null 2>&1
$BIN split-window -v -l 5 -t sz2:0 -P -F '#{pane_height}' >"$TMPDIR/o15" 2>&1
check "-l works for a stacked split" '^5$' "$TMPDIR/o15"
$BIN new-session -d -s sz3 >/dev/null 2>&1
$BIN split-window -h -p 50 -t sz3:0 -P -F '#{pane_width}' >"$TMPDIR/o16" 2>&1
check "-p is a share of the room" '^(39|40)$' "$TMPDIR/o16"
$BIN split-window -h -l 25% -t sz3:0.0 -P -F '#{pane_width}' >"$TMPDIR/o17" 2>&1
check "-l takes a percentage too" '^(9|10|11)$' "$TMPDIR/o17"
$BIN split-window -l ten -t sz3:0 >"$TMPDIR/o18" 2>&1
assert "a bad size is an error" '[ $? -ne 0 ] && grep -aq "bad size" "$TMPDIR/o18"'

echo "-- respawn-pane"
$BIN new-session -d -s rs 'echo first-run; sleep 60' >/dev/null 2>&1
sleep 1
$BIN display-message -p -t rs: '#{pane_id}' >"$TMPDIR/o19" 2>&1
$BIN respawn-pane -t rs: >"$TMPDIR/o20" 2>&1
assert "a running pane needs -k" '[ $? -ne 0 ] && grep -aq -- "-k" "$TMPDIR/o20"'
$BIN respawn-pane -k -t rs: 'echo second-run-$((2+3)); sleep 60' >"$TMPDIR/o21" 2>&1
assert "respawn-pane -k succeeds" '[ $? -eq 0 ]'
assert "the new command runs" 'until_true "$BIN capture-pane -p -t rs: 2>/dev/null | grep -q \"^second-run-5\$\"" 40'
$BIN capture-pane -p -t rs: >"$TMPDIR/o22" 2>&1
assert "the old output is gone" '! grep -aq "^first-run" "$TMPDIR/o22"'
$BIN display-message -p -t rs: '#{pane_id}' >"$TMPDIR/o23" 2>&1
assert "the pane keeps its number" 'cmp -s "$TMPDIR/o19" "$TMPDIR/o23"'
$BIN respawn-window -k -t rs: 'echo window-again; sleep 60' >/dev/null 2>&1
assert "respawn-window does a window of one pane" 'until_true "$BIN capture-pane -p -t rs: 2>/dev/null | grep -q \"^window-again\$\"" 40'

echo "-- join-pane and move-pane"
$BIN new-session -d -s jn -n a >/dev/null 2>&1
$BIN new-window -t jn -n b >/dev/null 2>&1
$BIN new-window -t jn -n c >/dev/null 2>&1
assert "three windows to start" '[ "$(windows_of jn)" = 3 ]'
$BIN join-pane -s jn:b -t jn:a -h >"$TMPDIR/o24" 2>&1
assert "join-pane succeeds" '[ $? -eq 0 ]'
assert "the emptied window is gone" '[ "$(windows_of jn)" = 2 ]'
assert "and its pane sits in the target window" '[ "$(panes_of jn:a)" = 2 ]'
$BIN join-pane -s jn:a.0 -t jn:a.1 >"$TMPDIR/o25" 2>&1
assert "a pane cannot join its own window" '[ $? -ne 0 ]'
$BIN move-pane -s jn:a.1 -t jn:c -v -l 5 >"$TMPDIR/o26" 2>&1
assert "move-pane takes a size" '[ $? -eq 0 ] && [ "$(panes_of jn:c)" = 2 ] && [ "$(panes_of jn:a)" = 1 ]'
$BIN new-session -d -s only >/dev/null 2>&1
$BIN join-pane -s only: -t jn:a >"$TMPDIR/o27" 2>&1
assert "the only pane of the only window cannot leave" '[ $? -ne 0 ]'

echo "-- wait-for"
rm -f "$TMPDIR/woke1" "$TMPDIR/woke2" "$TMPDIR/woke3"
($BIN wait-for chan1; touch "$TMPDIR/woke1") >/dev/null 2>&1 &
sleep 1
assert "a wait blocks" '[ ! -e "$TMPDIR/woke1" ]'
$BIN wait-for -S chan1 >/dev/null 2>&1
assert "a signal lets it go" 'until_true "[ -e \"$TMPDIR/woke1\" ]" 20'
$BIN wait-for -S early >/dev/null 2>&1
timeout 10 $BIN wait-for early >/dev/null 2>&1
assert "a signal sent before the wait is kept for it" '[ $? -eq 0 ]'
$BIN wait-for -L mtx >/dev/null 2>&1
($BIN wait-for -L mtx; touch "$TMPDIR/woke2") >/dev/null 2>&1 &
sleep 1
assert "a second lock waits" '[ ! -e "$TMPDIR/woke2" ]'
$BIN wait-for -U mtx >/dev/null 2>&1
assert "unlocking hands the lock on" 'until_true "[ -e \"$TMPDIR/woke2\" ]" 20'
$BIN wait-for -U mtx >/dev/null 2>&1
$BIN wait-for -U mtx >"$TMPDIR/o28" 2>&1
assert "unlocking a free channel is an error" '[ $? -ne 0 ] && grep -aq "not locked" "$TMPDIR/o28"'
$BIN wait-for 'bad name' >"$TMPDIR/o29" 2>&1
assert "a bad channel name is an error" '[ $? -ne 0 ]'
$BIN new-session -d -s wf >/dev/null 2>&1
($BIN wait-for from-pane; touch "$TMPDIR/woke3") >/dev/null 2>&1 &
sleep 1
$BIN send-keys -t wf: "$BIN wait-for -S from-pane" Enter >/dev/null 2>&1
assert "a program in a pane can signal" 'until_true "[ -e \"$TMPDIR/woke3\" ]" 40'

echo "-- pipe-pane"
$BIN new-session -d -s pp >/dev/null 2>&1
sleep 1
$BIN pipe-pane -t pp: "cat > $TMPDIR/piped.txt" >/dev/null 2>&1
$BIN send-keys -t pp: 'echo piped-marker-1' Enter >/dev/null 2>&1
assert "pane output reaches the command" 'until_true "grep -aq piped-marker-1 \"$TMPDIR/piped.txt\" 2>/dev/null" 40'
$BIN pipe-pane -o -t pp: "cat > $TMPDIR/ignored.txt" >/dev/null 2>&1
$BIN send-keys -t pp: 'echo piped-marker-2' Enter >/dev/null 2>&1
assert "-o leaves the pipe that is open" 'until_true "grep -aq piped-marker-2 \"$TMPDIR/piped.txt\" 2>/dev/null" 40 && [ ! -e "$TMPDIR/ignored.txt" ]'
$BIN pipe-pane -t pp: >/dev/null 2>&1
sleep 1
$BIN send-keys -t pp: 'echo after-close' Enter >/dev/null 2>&1
sleep 1
assert "no command closes the pipe" '! grep -aq after-close "$TMPDIR/piped.txt"'
$BIN pipe-pane -I -t pp: "printf 'echo typed-by-pipe-%s\\n' 7" >/dev/null 2>&1
assert "-I types the output of the command into the pane" 'until_true "$BIN capture-pane -p -t pp: 2>/dev/null | grep -q \"^typed-by-pipe-7\$\"" 40'

echo "-- command-alias"
$BIN set -g command-alias "ll=list-sessions -F '#{session_name}'" >/dev/null 2>&1
$BIN ll >"$TMPDIR/o30" 2>&1
check "an alias runs its command" '^base$' "$TMPDIR/o30"
$BIN set -g 'command-alias[7]' 'lp=list-panes -a -F "#{pane_id}"' >/dev/null 2>&1
$BIN lp >"$TMPDIR/o31" 2>&1
check "the indexed spelling works" '^%[0-9]+$' "$TMPDIR/o31"
$BIN set -g command-alias 'list-panes=list-sessions' >"$TMPDIR/o32" 2>&1
assert "an alias cannot hide a command" '[ $? -ne 0 ] && grep -aq "is a command already" "$TMPDIR/o32"'
$BIN set -gu 'command-alias[7]' >/dev/null 2>&1
$BIN lp >"$TMPDIR/o33" 2>&1
assert "unsetting a slot removes its alias" '[ $? -ne 0 ]'
$BIN ll >/dev/null 2>&1
assert "the others stay" '[ $? -eq 0 ]'
$BIN set -g command-alias 'bad name=x' >"$TMPDIR/o34" 2>&1
assert "a bad alias name is an error" '[ $? -ne 0 ]'

echo "-- capture-pane -e and -N"
$BIN new-session -d -s cp >/dev/null 2>&1
sleep 1
$BIN send-keys -t cp: "printf '\\033[31mred-text\\033[0m plain\\n'" Enter >/dev/null 2>&1
assert "colors are there" 'until_true "$BIN capture-pane -p -e -t cp: 2>/dev/null | grep -q red-text" 40'
$BIN capture-pane -p -e -t cp: >"$TMPDIR/o35" 2>&1
check "-e writes the escape sequence for them" $'\x1b\\[0;31mred-text' "$TMPDIR/o35"
$BIN capture-pane -p -t cp: >"$TMPDIR/o36" 2>&1
assert "without -e there are none" '! grep -aq "$(printf "\033")" "$TMPDIR/o36"'
$BIN capture-pane -p -N -t cp: >"$TMPDIR/o37" 2>&1
assert "-N keeps the blanks at the end of the lines" '[ "$(awk "{print length}" "$TMPDIR/o37" | sort -n | tail -1)" -ge 79 ]'

echo "-- list-keys -T"
$BIN list-keys -T copy-mode-vi >"$TMPDIR/o38" 2>&1
assert "only the table asked for" '[ -s "$TMPDIR/o38" ] && ! grep -aqv -- "-T copy-mode-vi" "$TMPDIR/o38"'
$BIN list-keys -T nonsense >"$TMPDIR/o39" 2>&1
assert "an unknown table is an error" '[ $? -ne 0 ] && grep -aq "unknown key table" "$TMPDIR/o39"'

echo "-- kill-pane -a and kill-window -a"
$BIN new-session -d -s kl -n k1 >/dev/null 2>&1
$BIN split-window -t kl:0 >/dev/null 2>&1
$BIN split-window -t kl:0 >/dev/null 2>&1
assert "three panes" '[ "$(panes_of kl:0)" = 3 ]'
$BIN kill-pane -a -t kl:0.0 >/dev/null 2>&1
assert "kill-pane -a keeps only the target" 'until_true "[ \"\$(panes_of kl:0)\" = 1 ]" 20'
$BIN new-window -t kl -n k2 >/dev/null 2>&1
$BIN new-window -t kl -n k3 >/dev/null 2>&1
$BIN kill-window -a -t kl:k2 >/dev/null 2>&1
assert "kill-window -a keeps only the target" 'until_true "[ \"\$(windows_of kl)\" = 1 ]" 20'
$BIN list-windows -t kl -F '#{window_name}' >"$TMPDIR/o40" 2>&1
check "and it is the one named" '^k2$' "$TMPDIR/o40"

$BIN kill-server >/dev/null 2>&1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
