#!/bin/bash
# #(command) in the status line (M37): the output shows up, a command that hangs is stopped and
# does not freeze the bar, escapes and # in the output are text, a name a pane could have set is
# refused (or quoted with q:), the output is refreshed every status-interval, and
# status-commands-max limits the commands.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-status.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export SHELL=/bin/sh
fail=0
T="$TMPDIR"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

mkdir -p "$T/dir with space"
cat >"$T/esc.sh" <<'EOF'
#!/bin/sh
printf '\033[31mESC:#[bg=red]#{session_name}#(echo hacked)\n'
EOF
chmod +x "$T/esc.sh"

cat >"$T/one.conf" <<EOF
set -g automatic-rename off
set -g status-interval 1
set -g status-command-timeout 1
set -g status-left 'L#(echo LEFTSEG)#(touch $T/refused.out #{session_name})'
set -g status-right '#(echo RIGHTSEG) #(sleep 3137; echo LATESEG) ENDBAR #(basename #{q:pane_current_path}) #($T/esc.sh) #(echo x >> $T/runs.out)'
set -g status-right-length 120
EOF

echo "-- the bar"
(sleep 6; printf '\002d'; sleep 1) | IMUX_CONFIG="$T/one.conf" timeout 40 script -q "$T/screen.out" sh -c "cd '$T/dir with space'; stty rows 24 cols 160; $BIN new -s f" >/dev/null
assert "the output of a command in status-left shows" 'grep -aq "LEFTSEG" "$T/screen.out"'
assert "the output of a command in status-right shows" 'grep -aq "RIGHTSEG" "$T/screen.out"'
assert "the rest of the bar is drawn while a command hangs" 'grep -aq "ENDBAR" "$T/screen.out"'
assert "a command that hangs shows nothing" '! grep -aq "LATESEG" "$T/screen.out"'
assert "the server is still there" '$BIN has-session -t f'
runs=$(wc -l <"$T/runs.out" 2>/dev/null | tr -d ' ')
sleep 5
later=$(wc -l <"$T/runs.out" | tr -d ' ')
sleep 3
assert "it is stopped with its children and, once nobody looks, not started again" '! pgrep -f "sleep 3137" >/dev/null'
assert "a bar nobody looks at any more is not refreshed ($later runs, then $(wc -l <"$T/runs.out" | tr -d ' '))" '[ "$(wc -l <"$T/runs.out" | tr -d " ")" = "$later" ]'
assert "the text of a command takes the directory of the pane, quoted with q:" 'grep -aq "dir with space" "$T/screen.out"'
assert "an escape in the output is dropped" 'grep -aq "ESC:#\[bg=red\]#{session_name}#(echo hacked)" "$T/screen.out"'
assert "and nothing in it ran as a command" '! grep -aq "hacked$" "$T/screen.out"'
assert "a name a pane could have set is refused and the bar says so" 'grep -aq "\[#{q:session_name}?\]" "$T/screen.out"'
assert "and the refused command did not run" '[ ! -e "$T/refused.out" ]'
assert "the output is refreshed every status-interval (ran $runs times)" '[ "${runs:-0}" -ge 3 ] && [ "${runs:-0}" -le 9 ]'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- the limit"
cat >"$T/two.conf" <<EOF
set -g automatic-rename off
set -g status-interval 1
set -g status-commands-max 1
set -g status-left '#(echo ONE >> $T/one.out)'
set -g status-right '#(echo TWO >> $T/two.out)'
EOF
(sleep 4; printf '\002d'; sleep 1) | IMUX_CONFIG="$T/two.conf" timeout 40 script -q "$T/screen2.out" sh -c "stty rows 24 cols 80; $BIN new -s f" >/dev/null
assert "the first command runs" '[ -s "$T/one.out" ]'
assert "a second different one is over status-commands-max" '[ ! -e "$T/two.out" ]'
$BIN kill-server >/dev/null 2>&1

echo "-- no client, no commands"
cat >"$T/three.conf" <<EOF
set -g automatic-rename off
set -g status-interval 1
set -g status-left '#(echo NOBODY >> $T/nobody.out)'
EOF
IMUX_CONFIG="$T/three.conf" $BIN new-session -d -s g >/dev/null 2>&1
sleep 3
assert "nothing runs for a bar nobody looks at" '[ ! -e "$T/nobody.out" ]'
$BIN kill-server >/dev/null 2>&1

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
