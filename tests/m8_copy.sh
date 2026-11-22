#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-copy.XXXXXX)
export IMUX_CONFIG="$TMPDIR/conf" IMUX_STATE_DIR="$TMPDIR/state"
printf 'set copy-command ""\nset mode-keys vi\n' >"$IMUX_CONFIG"
fail=0

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 40 script -q "$out" sh -c "stty rows 30 cols 100; $BIN"
}

(sleep 14; $BIN kill-server >/dev/null 2>&1) &
run_client "$TMPDIR/a.out" \
  'seq 1 60\r' \
  '\002[' \
  'g' \
  'q' \
  'echo copyme-$((6*7))\r' \
  '\002[' \
  '?copyme-4\r' \
  'v$y' \
  'echo pasted:' \
  '\002]' \
  '\r' \
  '\033[<64;10;5M' \
  '\033[<0;10;5M\033[<0;10;5m\033[<0;10;5M\033[<0;10;5m' \
  'echo dbl-$((6*7))\r'

check "copy mode shows a position indicator" '\[[0-9][0-9]*/[0-9][0-9]*\]' "$TMPDIR/a.out"
check "wheel up enters copy mode and scrolls five lines" '\[5/' "$TMPDIR/a.out"
check "yank then paste buffer types the copied text" "pasted:copyme-42" "$TMPDIR/a.out"
check "a double click leaves copy mode so typing reaches the shell again" "dbl-42" "$TMPDIR/a.out"

sleep 1
grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
