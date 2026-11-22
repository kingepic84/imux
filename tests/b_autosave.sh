#!/bin/bash
# The autosave is written on a thread of its own: while a (deliberately slow) write is running the
# server still answers at once, the file arrives in the end, and kill-server never leaves one behind
# even when it comes while a write is in flight. Needs a debug build (IMUX_TEST_SAVE_DELAY_MS).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-autosave.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export IMUX_TEST_SAVE_DELAY_MS=3000
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"
SAVE="$IMUX_STATE_DIR/autosave.imux"

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

now_ms() {
  perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'
}

printf 'set autosave-interval 1\n' >"$IMUX_CONFIG"

(sleep 90; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

(sleep 1; sleep 40) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s w" >/dev/null 2>&1 &
CLIENT=$!
until_true '$BIN ls 2>/dev/null | grep -q "^w:"' 40
assert "the server is up" '$BIN ls 2>/dev/null | grep -q "^w:"'

echo "-- a slow write does not stall the loop"
$BIN new-window -t w -n changed >/dev/null 2>&1
worst=0
start=$(now_ms)
while [ $(($(now_ms) - start)) -lt 4500 ]; do
  t0=$(now_ms)
  $BIN ls >/dev/null 2>&1
  took=$(($(now_ms) - t0))
  [ "$took" -gt "$worst" ] && worst=$took
  sleep 0.1
done
echo "     slowest answer while the write ran: ${worst} ms"
assert "the server answered within a second throughout" '[ "$worst" -lt 1000 ]'
assert "the autosave file arrived" 'until_true "[ -s \"$SAVE\" ]" 40'

echo "-- kill-server while a write is in flight"
$BIN new-window -t w -n again >/dev/null 2>&1
sleep 2
$BIN kill-server >/dev/null 2>&1
assert "the server ends" 'until_true "$BIN ls 2>&1 | grep -q \"no server running\"" 60'
assert "and no autosave is left behind" '[ ! -e "$SAVE" ] && [ ! -e "$IMUX_STATE_DIR/autosave.prev" ]'

wait $CLIENT 2>/dev/null
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
