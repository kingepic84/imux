#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-statusrows.XXXXXX)
export IMUX_CONFIG=/dev/null IMUX_STATE_DIR="$TMPDIR/state"
fail=0

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 40 script -q "$out" sh -c "stty rows 30 cols 60; $BIN"
}

many='\002c\002c\002c\002c\002c\002c\002c\002c\002c\002c\002c'
kills='\002&\002&\002&\002&\002&\002&\002&\002&\002&\002&\002&'

run_client "$TMPDIR/a.out" \
  'echo one-row-$(stty size | tr " " x)\r' \
  "$many" \
  'echo wrapped-$(stty size | tr " " x)\r' \
  "$kills" \
  'echo back-$(stty size | tr " " x)\r' \
  'exit\r'

check "one window: single status row" "one-row-29x60" "$TMPDIR/a.out"
check "twelve windows: tabs wrap and the panes shrink" "wrapped-2[0-8]x60" "$TMPDIR/a.out"
check "back to one window: panes grow back" "back-29x60" "$TMPDIR/a.out"
check "clock stays visible while tabs wrap" "[0-9][0-9]:[0-9][0-9] [0-9][0-9]-[A-Z][a-z][a-z]-[0-9][0-9]" "$TMPDIR/a.out"

sleep 1
if [ ! -e "$TMPDIR/imux-$(id -u)/default" ]; then echo "ok   server exits when the last window closes"; else echo "FAIL server still running"; fail=1; fi

grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
