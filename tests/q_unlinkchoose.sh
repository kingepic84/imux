#!/bin/bash
# unlink-window (a window is never in two sessions, so it is kill-window), and choose-window and
# choose-session as the names of the two pickers.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-quc.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' 'bind W choose-window' 'bind S choose-session' 'bind U unlink-window' >"$IMUX_CONFIG"

order() { $BIN list-windows -t "$1" -F '#{window_name}' 2>/dev/null | tr '\n' ' '; }
$BIN new-session -d -s uq -n a >/dev/null 2>&1
for n in b c d; do $BIN new-window -d -t uq -n "$n" >/dev/null 2>&1; done

$BIN unlink-window -t uq:b >"$TMPDIR/o1" 2>&1
assert "unlink-window removes the window" '[ "$(order uq)" = "a c d " ]'
$BIN unlink-window -k -t uq:c >"$TMPDIR/o2" 2>&1
assert "-k is accepted" '[ "$(order uq)" = "a d " ]'
$BIN unlinkw -t uq:d >"$TMPDIR/o3" 2>&1
assert "the short name works" '[ "$(order uq)" = "a " ]'

$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "the three bindings loaded" 'grep -aq "choose" "$TMPDIR/keys" && grep -aq "unlink-window\|kill-window" "$TMPDIR/keys"'

client() {
  (sleep 2; "$@"; sleep 1; printf '\033'; sleep 0.5; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t uq" >/dev/null 2>&1
}
k_window() { printf '\002W'; sleep 0.8; }
client k_window
assert "choose-window opens the window picker" 'LC_ALL=C grep -aq "sessions and windows" "$TMPDIR/c.out"'
k_session() { printf '\002S'; sleep 0.8; }
client k_session
assert "choose-session opens the session list, not the tree" 'LC_ALL=C grep -aq "sessions" "$TMPDIR/c.out" && ! LC_ALL=C grep -aq "sessions and windows" "$TMPDIR/c.out"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
