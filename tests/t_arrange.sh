#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-arrange.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) &

SIZE='echo SZ-$(stty size | tr " " x)\r'

echo "-- zoom"
ARGS="new -s w" HOLD=1 client "$TMPDIR/a.out" \
  '\002%' \
  'echo before-$(stty size | tr " " x)\r' \
  '\002z' \
  'echo zoomed-$(stty size | tr " " x)\r' \
  '\002z' \
  'echo after-$(stty size | tr " " x)\r' >/dev/null
check "the right pane is half the window before" 'before-29x49' "$TMPDIR/a.out"
check "zoom gives it the whole window" 'zoomed-29x100' "$TMPDIR/a.out"
check "the status line marks the zoomed window (the Z is the one cell that changes)" '\[[0-9]+;[0-9]+H.\[[0-9;]*mZ' "$TMPDIR/a.out"
check "zoom again gives the half back" 'after-29x49' "$TMPDIR/a.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- break-pane"
ARGS="new -s w" HOLD=1 client "$TMPDIR/b.out" \
  '\002%' \
  '\002!' \
  'echo broken-$(stty size | tr " " x)\r' >/dev/null
check "the broken out pane has a window to itself" 'broken-29x100' "$TMPDIR/b.out"
assert "so there are two windows" '[ "$($BIN ls | grep -o "[0-9]* window" | head -1)" = "2 window" ]'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- swap-pane and rotate-window"
ARGS="new -s w" HOLD=1 client "$TMPDIR/c.out" \
  '\002%' \
  'echo right-$(stty size | tr " " x)\r' \
  '\002{' \
  'echo swapped-$(stty size | tr " " x)\r' \
  '\002}' \
  'echo back-$(stty size | tr " " x)\r' \
  '\x0f' \
  'echo rotated-$(stty size | tr " " x)\r' >/dev/null
check "the right pane is 49 wide" 'right-29x49' "$TMPDIR/c.out"
check "swap-pane -U moves the active pane to the left, where there are 50 columns" 'swapped-29x50' "$TMPDIR/c.out"
check "swap-pane -D moves it back" 'back-29x49' "$TMPDIR/c.out"
$BIN kill-server >/dev/null 2>&1
sleep 1
ARGS="new -s w" HOLD=1 client "$TMPDIR/d.out" \
  '\002%' \
  '\002\x0f' \
  'echo rotated-$(stty size | tr " " x)\r' >/dev/null
check "rotate-window moves the active pane to the other place" 'rotated-29x50' "$TMPDIR/d.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- layouts"
ARGS="new -s w" HOLD=1 client "$TMPDIR/e.out" \
  '\002%' '\002%' \
  '\002\x1b4' \
  'echo mainv-$(stty size | tr " " x)\r' \
  '\002\x1b5' \
  'echo tiled-$(stty size | tr " " x)\r' \
  '\002\x1b1' \
  'echo evenh-$(stty size | tr " " x)\r' \
  '\002 ' \
  'echo next-$(stty size | tr " " x)\r' >/dev/null
check "main-vertical puts the last pane under the other small one" 'mainv-14x33' "$TMPDIR/e.out"
check "tiled gives three panes a 2 by 2 grid with the last one wide" 'tiled-14x100' "$TMPDIR/e.out"
check "even-horizontal makes three columns" 'evenh-29x3[0-3]' "$TMPDIR/e.out"
check "Space goes on to the next preset, even-vertical" 'next-(9|10)x100' "$TMPDIR/e.out"

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
