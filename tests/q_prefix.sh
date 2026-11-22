#!/bin/bash
# `#{client_prefix}`: 1 while the client has pressed the prefix and not yet the key after it, in the
# status line and in display-message; the bar redraws when the prefix is pressed and when it is used.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qx.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' \
  'set -g status-right "[#{?client_prefix,PFXON,PFXOFF}]"' \
  'bind e display-message -p "ONKEY=#{client_prefix}"' \
  >"$IMUX_CONFIG"

$BIN new-session -d -s pfx -n start >/dev/null 2>&1
$BIN check-config "$IMUX_CONFIG" >"$TMPDIR/check" 2>&1
assert "check-config knows client_prefix" '! grep -aq "client_prefix" "$TMPDIR/check"'

assert "no client, no prefix" '[ "$($BIN display-message -p "#{client_prefix}" 2>/dev/null)" = 0 ]'

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t pfx" >/dev/null 2>&1
}

k_press() { printf '\002'; sleep 1.2; printf 'e'; sleep 0.8; }
client k_press
assert "the bar starts with the prefix off" 'LC_ALL=C grep -aq "PFXOFF" "$TMPDIR/c.out"'
assert "the bar redraws with the prefix on after C-b" 'LC_ALL=C grep -aq "PFXON" "$TMPDIR/c.out"'

: >"$TMPDIR/c.out"
k_plain() { sleep 1; }
client k_plain
assert "without C-b the bar never shows the prefix on" '! LC_ALL=C grep -aq "PFXON" "$TMPDIR/c.out"'

$BIN new-window -d -n cmdw 'sleep 60' >/dev/null 2>&1
assert "pane_start_command is the command the pane was started with" '[ "$($BIN display-message -p -t pfx:cmdw "#{pane_start_command}" 2>/dev/null)" = "sleep 60" ]'
assert "a plain shell pane has no start command" '[ -z "$($BIN display-message -p -t pfx:start "#{pane_start_command}" 2>/dev/null)" ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
