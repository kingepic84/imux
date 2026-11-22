#!/bin/bash
# attach -d, new -A, new -A -D, and detach-client -s / -t / -a -t on the socket.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qaf.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s ws -n one >/dev/null 2>&1
$BIN new-session -d -s other -n two >/dev/null 2>&1

hold() {
  local tag=$1 secs=$2
  shift 2
  (sleep "$secs"; printf '\002d'; sleep 1) | timeout 60 script -q "$TMPDIR/$tag.out" sh -c "stty rows 30 cols 100; $BIN $*" >/dev/null 2>&1 &
}
count() { $BIN list-clients -F '#{client_session}' 2>/dev/null | grep -c "^$1\$"; }
total() { $BIN list-clients -F x 2>/dev/null | grep -c x; }

hold a 14 attach -t ws
sleep 2
assert "one client on ws" '[ "$(count ws)" = 1 ]'

hold b 10 attach -d -t ws
sleep 3
assert "attach -d leaves only the new client" '[ "$(count ws)" = 1 ]'
assert "the first client was told it is detached" 'LC_ALL=C grep -aq "detached" "$TMPDIR/a.out"'

hold c 8 new -A -s ws
sleep 2
assert "new -A -s of an existing name attaches" '[ "$(count ws)" = 2 ]'
assert "and makes no second session" '[ "$($BIN list-sessions -F x | grep -c x)" = 2 ]'

hold d 6 new -A -D -s ws
sleep 3
assert "new -A -D attaches and detaches the others" '[ "$(count ws)" = 1 ]'

hold e 6 new -A -s fresh
sleep 2
assert "new -A -s of a new name creates it" '$BIN has-session -t =fresh && [ "$(count fresh)" = 1 ]'

assert "a flag the form does not know is refused" '! $BIN attach -t ws --bogus >"$TMPDIR/bad" 2>&1 && grep -q "unexpected argument" "$TMPDIR/bad"'

sleep 9
assert "the clients are gone when their terminals close" '[ "$(total)" = 0 ]'

hold f 20 attach -t ws
hold g 20 attach -t ws
hold h 20 attach -t other
sleep 3
assert "three clients" '[ "$(total)" = 3 ]'

$BIN detach-client -s other >/dev/null 2>&1
sleep 1
assert "detach-client -s detaches every client of that session only" '[ "$(count other)" = 0 ] && [ "$(count ws)" = 2 ]'

keep=$($BIN list-clients -F '#{client_name}' | head -1)
$BIN detach-client -a -t "$keep" >/dev/null 2>&1
sleep 1
assert "detach-client -a -t keeps just that client" '[ "$(total)" = 1 ] && [ "$($BIN list-clients -F "#{client_name}")" = "$keep" ]'

$BIN detach-client -t "$keep" >/dev/null 2>&1
sleep 1
assert "detach-client -t detaches the named client" '[ "$(total)" = 0 ]'

assert "an unknown client is an error" '! $BIN detach-client -t client-9999 >"$TMPDIR/bad2" 2>&1 && grep -q "can.t find client" "$TMPDIR/bad2"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
