#!/bin/bash
# Commands a key can run that have no typed form (send-keys, new-window -a/-d/-n, set, ...) and the
# questions of `command-prompt` (template, -I, -1) and `confirm-before`: each is driven by a bound
# key through a `script` client.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qb.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 200; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' \
  'bind a command-prompt -p "name:" "rename-window %%"' \
  'bind i command-prompt -I pre -p "x:" "rename-window %%"' \
  'bind o command-prompt -1 -p "one:" "rename-window k%%"' \
  'bind y confirm-before -p "sure?" "rename-window sure"' \
  'bind w new-window -a -n after' \
  'bind b new-window -d -n bg' \
  'bind e set -g @marker yes' \
  'bind s send-keys "echo ran-$((20+22))" Enter' \
  'bind H clear-history' \
  "bind r run \"echo run-in-key > $TMPDIR/runkey.out\"" \
  >"$IMUX_CONFIG"

$BIN new-session -d -s bnd -n start >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "the bindings loaded" 'grep -aq "confirm-before" "$TMPDIR/keys" && grep -aq "new-window -a" "$TMPDIR/keys"'

names() { $BIN list-windows -F '#{window_name}' 2>/dev/null | tr '\n' ' '; }
current() { $BIN display-message -p '#{window_name}' 2>/dev/null; }

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t bnd" >/dev/null 2>&1
}

k_template() { printf '\002a'; sleep 0.5; printf 'typed\r'; }
client k_template
assert "what is typed replaces %% in the template" '[ "$(current)" = typed ]'
assert "the label is drawn" 'LC_ALL=C grep -aq "name:" "$TMPDIR/c.out"'

k_cancel() { printf '\002a'; sleep 0.5; printf 'nope'; sleep 0.3; printf '\033'; }
client k_cancel
assert "Escape cancels and nothing runs" '[ "$(current)" = typed ]'

k_initial() { printf '\002i'; sleep 0.5; printf '\r'; }
client k_initial
assert "the initial text is what runs when Enter is pressed at once" '[ "$(current)" = pre ]'

k_single() { printf '\002o'; sleep 0.5; printf 'z'; }
client k_single
assert "-1 takes one key and runs without Enter" '[ "$(current)" = kz ]'

k_no() { printf '\002y'; sleep 0.5; printf 'n'; }
client k_no
assert "a confirmation answered with anything but y does nothing" '[ "$(current)" = kz ]'

k_yes() { printf '\002y'; sleep 0.5; printf 'y'; }
client k_yes
assert "a confirmation answered with y runs the command" '[ "$(current)" = sure ]'
assert "the confirmation shows its label" 'LC_ALL=C grep -aq "sure?" "$TMPDIR/c.out"'

$BIN select-window -t :0 >/dev/null 2>&1
k_after() { printf '\002w'; sleep 0.7; }
client k_after
assert "new-window -a puts the window after the current one" '[ "$($BIN list-windows -F "#{window_name}" | sed -n 2p)" = after ] && [ "$(current)" = after ]'

before=$($BIN list-windows | wc -l | tr -d ' ')
k_bg() { printf '\002b'; sleep 0.7; }
client k_bg
now=$($BIN list-windows | wc -l | tr -d ' ')
assert "new-window -d makes a window and does not go to it" '[ "$now" = "$((before + 1))" ] && [ "$(current)" != bg ]'

k_set() { printf '\002e'; sleep 0.7; }
client k_set
marker=$($BIN show-options -gv @marker 2>/dev/null)
assert "set -g in a binding sets the option" '[ "$marker" = yes ]'

k_send() { printf '\002s'; sleep 1.5; }
client k_send
sleep 0.5
assert "send-keys in a binding types into the pane" '$BIN capture-pane -p | grep -aq "ran-42"'

k_run() { printf '\002r'; sleep 1.5; }
client k_run
assert "run in a binding is run-shell" '[ "$(cat $TMPDIR/runkey.out 2>/dev/null)" = run-in-key ]'

$BIN select-window -t bnd:0 >/dev/null 2>&1
$BIN send-keys -t %0 C-u >/dev/null 2>&1
$BIN send-keys -t %0 -l 'seq 1 300' >/dev/null 2>&1
$BIN send-keys -t %0 Enter >/dev/null 2>&1
sleep 1.5
lines() { $BIN capture-pane -p -S -1000 -t %0 | grep -ac '^[0-9][0-9]*$'; }
before=$(lines)
assert "the scrollback holds the numbers" '[ "$before" -gt 100 ]'
k_clear() { printf '\002H'; sleep 0.7; }
client k_clear
after=$(lines)
assert "clear-history empties the scrollback and keeps the screen" '[ "$after" -lt 40 ] && [ "$after" -gt 0 ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
