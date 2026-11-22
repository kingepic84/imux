#!/bin/bash
# The "all but this" and "keep where I am" flags: `kill-session -a`, `kill-window -a` and
# `break-pane -d` from a bound key, `display-panes -d` and `detach-client -a` as bindings.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qa.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' \
  'bind K kill-window -a' \
  'bind B break-pane -d' \
  'bind P display-panes -d 1500' \
  'bind D detach-client -a' \
  'bind X kill-session -a' \
  >"$IMUX_CONFIG"

$BIN new-session -d -s keep -n one >/dev/null 2>&1
$BIN new-session -d -s other1 >/dev/null 2>&1
$BIN new-session -d -s other2 >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "the bindings loaded and describe themselves" 'grep -aq "kill-window -a" "$TMPDIR/keys" && grep -aq "break-pane -d" "$TMPDIR/keys" && grep -aq "display-panes -d 1500" "$TMPDIR/keys" && grep -aq "detach-client -a" "$TMPDIR/keys" && grep -aq "kill-session -a" "$TMPDIR/keys"'

$BIN kill-session -a -t keep >"$TMPDIR/ks" 2>&1
sleep 1
sessions=$($BIN list-sessions -F '#{session_name}' 2>/dev/null | tr '\n' ' ')
assert "kill-session -a -t keeps the target and ends the rest" '[ "$sessions" = "keep " ]'

$BIN new-window -t keep -n two >/dev/null 2>&1
$BIN new-window -t keep -n three >/dev/null 2>&1
$BIN select-window -t keep:1 >/dev/null 2>&1
$BIN split-window -d -t keep:1 >/dev/null 2>&1
count() { $BIN list-windows -t keep -F '#{window_name}' 2>/dev/null | wc -l | tr -d ' '; }
current() { $BIN display-message -p -t keep: '#{window_name}' 2>/dev/null; }
assert "three windows, the second current" '[ "$(count)" = 3 ] && [ "$(current)" = two ]'

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t keep" >/dev/null 2>&1
}

k_break() { printf '\002B'; sleep 0.7; }
client k_break
assert "break-pane -d makes a window and stays in the current one" '[ "$(count)" = 4 ] && [ "$(current)" = two ]'

k_kill() { printf '\002K'; sleep 1; }
client k_kill
assert "kill-window -a ends every window but the current one" '[ "$(count)" = 1 ] && [ "$(current)" = two ]'

$BIN new-session -d -s extra >/dev/null 2>&1
k_x() { printf '\002X'; sleep 1; }
client k_x
sessions=$($BIN list-sessions -F '#{session_name}' 2>/dev/null | tr '\n' ' ')
assert "kill-session -a from a key ends the other sessions" '[ "$sessions" = "keep " ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
