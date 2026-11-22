#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-stock.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aqE -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) &

echo "-- the prefix is C-b, windows start at 0 and the bar is bare"
ARGS="new -s w" HOLD=1 client "$TMPDIR/a.out" '\002c' '\002' '0' 'echo id-$IMUX_PANE_ID\r' '\002l' 'echo back-$IMUX_PANE_ID\r' >/dev/null
check "the status line shows the session in brackets" '\[w\]' "$TMPDIR/a.out"
check "the first window is 0" '0:[a-z]+' "$TMPDIR/a.out"
check "the second is 1 and the current one is starred" '1:[a-z]+\*' "$TMPDIR/a.out"
check "0 selects the first window" 'id-0' "$TMPDIR/a.out"
check "l goes back to the last window" 'back-1' "$TMPDIR/a.out"
assert "c created a second window" '[ "$($BIN ls | grep -o "[0-9]* window" | head -1)" = "2 window" ]'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- C-a is no prefix by default"
ARGS="new -s w" HOLD=1 client "$TMPDIR/b.out" '\001c' >/dev/null
assert "C-a c did not create a window" '[ "$($BIN ls | grep -o "[0-9]* window" | head -1)" = "1 window" ]'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- the two splits"
ARGS="new -s w" HOLD=1 client "$TMPDIR/c.out" '\002%' 'echo side-$IMUX_PANE_ID\r' '\002o' 'echo side-$IMUX_PANE_ID\r' >/dev/null
check "% opens a second pane next to the first" 'side-0' "$TMPDIR/c.out"
check "which has its own number" 'side-1' "$TMPDIR/c.out"
$BIN kill-server >/dev/null 2>&1
sleep 1
ARGS="new -s w" HOLD=1 client "$TMPDIR/d.out" '\002"' 'echo stack-$IMUX_PANE_ID\r' '\002o' 'echo stack-$IMUX_PANE_ID\r' >/dev/null
check '" opens a second pane below the first' 'stack-0' "$TMPDIR/d.out"
check "which has its own number" 'stack-1' "$TMPDIR/d.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- the personal keys are not bound"
ARGS="new -s w" HOLD=1 client "$TMPDIR/e.out" '\002|' '\002-' 'echo only-$IMUX_PANE_ID\r' >/dev/null
check "| and - did not split" 'only-0' "$TMPDIR/e.out"
refute "so there is no pane 1" 'only-1' "$TMPDIR/e.out"

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
