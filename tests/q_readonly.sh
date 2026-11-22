#!/bin/bash
# attach -r: a client that only watches. What it types and its mouse are dropped, the keys that
# detach it still work, list-clients shows it; attach -x is accepted; a normal client still types.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qro.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

: >"$TMPDIR/typed"
$BIN new-session -d -s ws "cat >$TMPDIR/typed" >/dev/null 2>&1
sleep 1
total() { $BIN list-clients -F x 2>/dev/null | grep -c x; }
windows() { $BIN list-windows -t ws -F x 2>/dev/null | grep -c x; }

echo "-- a read-only client"
(sleep 4; printf 'hello\r'; printf '\033[<0;5;5M\033[<0;5;5m'; printf '\002c'; sleep 6; printf '\002d'; sleep 1) | timeout 60 script -q "$TMPDIR/ro.out" sh -c "stty rows 30 cols 100; $BIN attach -r -t ws" >/dev/null 2>&1 &
sleep 3
assert "it is attached" '[ "$(total)" = 1 ]'
assert "list-clients says read-only in the default format" '$BIN list-clients | grep -q "(read-only)"'
assert "#{client_readonly} is 1" '[ "$($BIN list-clients -F "#{client_readonly}")" = 1 ]'
assert "the filter can pick it" '[ "$($BIN list-clients -f "#{client_readonly}" -F x | grep -c x)" = 1 ]'
sleep 4
assert "what it typed did not reach the pane" '[ ! -s "$TMPDIR/typed" ]'
assert "the prefix and c did not open a window" '[ "$(windows)" = 1 ]'
assert "it is still attached" '[ "$(total)" = 1 ]'
sleep 5
assert "the prefix and d detached it" '[ "$(total)" = 0 ]'

echo "-- a normal client"
(sleep 3; printf 'world\r'; sleep 3; printf '\002d'; sleep 1) | timeout 60 script -q "$TMPDIR/rw.out" sh -c "stty rows 30 cols 100; $BIN attach -x -t ws" >/dev/null 2>&1 &
sleep 2
assert "attach -x is accepted" '[ "$(total)" = 1 ]'
assert "a normal client is not read-only" '[ "$($BIN list-clients -F "#{client_readonly}")" = 0 ] && ! $BIN list-clients | grep -q "(read-only)"'
sleep 3
assert "what it types reaches the pane" 'grep -q world "$TMPDIR/typed"'
sleep 3
assert "and it detaches" '[ "$(total)" = 0 ]'

echo "-- flags"
assert "an unknown attach flag is still refused" '! $BIN attach -t ws -q >"$TMPDIR/bad" 2>&1 && grep -q "unexpected argument" "$TMPDIR/bad"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
