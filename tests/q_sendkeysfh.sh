#!/bin/bash
# send-keys -H (bytes in hexadecimal) and -F (the words are formats), checked before anything is typed.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qsk.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

OUT="$TMPDIR/out"
$BIN new-session -d -s skeys "cat >$OUT" >/dev/null 2>&1
sleep 1

$BIN send-keys -t skeys: -H 68 69 0a >"$TMPDIR/o1" 2>&1
sleep 1
assert "-H types the bytes" '[ "$(command cat $OUT)" = hi ]'

$BIN send-keys -t skeys: -H 0x41 42 0a >"$TMPDIR/o2" 2>&1
sleep 1
assert "-H takes 0x forms" 'grep -q "^AB$" "$OUT"'

$BIN send-keys -t skeys: -H 43 zz >"$TMPDIR/o3" 2>&1
assert "-H with a word that is no byte is an error" 'grep -q "hexadecimal" "$TMPDIR/o3"'
$BIN send-keys -t skeys: -H 0a >/dev/null 2>&1
sleep 1
assert "and nothing of it was typed" '! grep -q C "$OUT"'

$BIN send-keys -t skeys: -H 100 >"$TMPDIR/o4" 2>&1
assert "a value over one byte is refused" 'grep -q "hexadecimal" "$TMPDIR/o4"'

$BIN send-keys -t skeys: -F -l '#{session_name}' >/dev/null 2>&1
$BIN send-keys -t skeys: Enter >/dev/null 2>&1
sleep 1
assert "-F expands the words" 'grep -q "^skeys$" "$OUT"'

$BIN send-keys -t skeys: -l '#{session_name}' >/dev/null 2>&1
$BIN send-keys -t skeys: Enter >/dev/null 2>&1
sleep 1
assert "without -F a format is typed as it is" 'grep -q "^#{session_name}$" "$OUT"'

$BIN send-keys -t skeys: -F 'x#{window_index}' Enter >/dev/null 2>&1
sleep 1
assert "-F keeps key names working" 'grep -q "^x0$" "$OUT"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
