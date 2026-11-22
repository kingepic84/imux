#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-windows.XXXXXX)
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

run_client "$TMPDIR/a.out" \
  'export W=one\r' \
  '\002c' \
  'export W=two\r' \
  'echo now-$W\r' \
  '\033[<0;8;30M\033[<0;8;30m' \
  'echo clicked-$W\r' \
  '\002n' \
  'echo next-$W\r' \
  '\002p' \
  'echo prev-$W\r' \
  '\002c' \
  'export W=three\r' \
  '\002' '1' \
  'echo number-$W\r' \
  '\002' '2' \
  '\002&' \
  'echo killed-$W\r' \
  'exit\r' \
  'exit\r'

check "new window starts a fresh shell" "now-two" "$TMPDIR/a.out"
check "clicking the first tab selects window 0" "clicked-one" "$TMPDIR/a.out"
check "prefix n goes to the next window" "next-two" "$TMPDIR/a.out"
check "prefix p goes back" "prev-one" "$TMPDIR/a.out"
check "prefix 1 selects window 1" "number-two" "$TMPDIR/a.out"
check "prefix & kills the window and returns to the previous one" "killed-two" "$TMPDIR/a.out"
check "status line shows the tab names" "zsh" "$TMPDIR/a.out"

sleep 1
if [ ! -e "$TMPDIR/imux-$(id -u)/default" ]; then echo "ok   server exits when the last window closes"; else echo "FAIL server still running"; fail=1; fi

grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
