#!/bin/bash
# Relative targets from a bound key: `swap-window -t -1|+1`, `select-pane -t :.+`, and the
# clustered flags of `choose-tree -Zw`.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qr.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' \
  'bind -n C-S-Left swap-window -t -1' \
  'bind -n C-S-Right swap-window -t +1' \
  'bind o select-pane -t :.+' \
  'bind T choose-tree -Zw' \
  >"$IMUX_CONFIG"

$BIN new-session -d -s rel -n a >/dev/null 2>&1
$BIN new-window -t rel -n b >/dev/null 2>&1
$BIN new-window -t rel -n c >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "the bindings loaded" 'grep -aq "swap-window -t -1" "$TMPDIR/keys" && grep -aq "swap-window -t +1" "$TMPDIR/keys" && grep -aq "select-pane -t :.+" "$TMPDIR/keys"'

order() { $BIN list-windows -t rel -F '#{window_name}' 2>/dev/null | tr '\n' ' '; }
current() { $BIN display-message -p -t rel: '#{window_name}' 2>/dev/null; }

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t rel" >/dev/null 2>&1
}

k_left() { printf '\033[1;6D'; sleep 0.7; }
client k_left
assert "swap-window -t -1 trades with the window before and follows it" '[ "$(order)" = "a c b " ] && [ "$(current)" = c ]'

client k_left
assert "again" '[ "$(order)" = "c a b " ] && [ "$(current)" = c ]'

client k_left
assert "at the first window it stays and nothing is lost" '[ "$(order)" = "c a b " ]'

k_right() { printf '\033[1;6C'; sleep 0.7; }
client k_right
assert "swap-window -t +1 trades with the window after" '[ "$(order)" = "a c b " ] && [ "$(current)" = c ]'

$BIN select-window -t rel:0 >/dev/null 2>&1
$BIN split-window -d -t rel:0 >/dev/null 2>&1
$BIN split-window -d -t rel:0 >/dev/null 2>&1
$BIN select-pane -t rel:0.0 >/dev/null 2>&1
pane() { $BIN display-message -p -t rel: '#{pane_index}' 2>/dev/null; }
assert "three panes, first active" '[ "$(pane)" = 0 ]'

k_next() { printf '\002o'; sleep 0.7; }
client k_next
assert "select-pane -t :.+ moves to the next pane" '[ "$(pane)" = 1 ]'
client k_next
client k_next
assert "and wraps after the last" '[ "$(pane)" = 0 ]'

k_tree() { printf '\002T'; sleep 0.7; printf '\033'; }
client k_tree
assert "choose-tree -Zw opens the window picker" 'LC_ALL=C grep -aq "sessions and windows" "$TMPDIR/c.out"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
