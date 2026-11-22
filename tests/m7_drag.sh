#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-drag.XXXXXX)
export IMUX_CONFIG=/dev/null IMUX_STATE_DIR="$TMPDIR/state"
fail=0

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 30 script -q "$out" sh -c "stty rows 30 cols 100; $BIN"
}

(sleep 9; $BIN kill-server >/dev/null 2>&1) &
run_client "$TMPDIR/a.out" \
  '\002%' \
  '\033[<0;51;5M\033[<32;41;5M\033[<32;31;5M\033[<0;31;5m' \
  'echo right-size-$(stty size | tr " " x)\r' \
  '\033[<0;5;5M\033[<0;5;5m' \
  'echo left-size-$(stty size | tr " " x)\r'

check "dragging the divider left narrows the left pane and widens the right one" "right-size-29x69" "$TMPDIR/a.out"
check "the left pane followed the pointer to 30 columns" "left-size-29x30" "$TMPDIR/a.out"

sleep 1
grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
