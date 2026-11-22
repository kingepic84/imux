#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-panes.XXXXXX)
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
  'echo first-size-$(stty size | tr " " x)\r' \
  '\002%' \
  'echo second-size-$(stty size | tr " " x)\r' \
  '\002\033[D' \
  'echo back-in-first-$((6*7))\r' \
  '\002"' \
  'echo stacked-size-$(stty size | tr " " x)\r' \
  '\002x' \
  'echo after-kill-$((5*5))\r' \
  '\002\033[1;3C' \
  'echo resized-size-$(stty size | tr " " x)\r' \
  '\002:' \
  'select-layout even-horizontal\r' \
  'echo even-size-$(stty size | tr " " x)\r' \
  '\002o' \
  'exit\r' \
  'exit\r' \
  'exit\r'

check "single pane is the full area minus status row" "first-size-29x100" "$TMPDIR/a.out"
check "side-by-side split halves the width (49 cols for the right pane)" "second-size-29x49" "$TMPDIR/a.out"
check "prefix left arrow returns to the first pane" "back-in-first-42" "$TMPDIR/a.out"
check "stacked split halves the height" "stacked-size-14x50" "$TMPDIR/a.out"
check "prefix x closes the pane and the next one keeps working" "after-kill-25" "$TMPDIR/a.out"
check "prefix Alt-Right grows the pane to the right" "resized-size-29x55" "$TMPDIR/a.out"
check "divider glyph drawn" "│" "$TMPDIR/a.out"

sleep 2
run_client "$TMPDIR/b.out" \
  '\002%' \
  '\033[<0;5;5M\033[<0;5;5m' \
  'echo clicked-size-$(stty size | tr " " x)\r' \
  '\033[<0;90;5M\033[<0;90;5m' \
  'echo clicked-right-size-$(stty size | tr " " x)\r' \
  'exit\r' \
  'exit\r'
check "clicking the left pane focuses it" "clicked-size-29x50" "$TMPDIR/b.out"
check "clicking the right pane focuses it" "clicked-right-size-29x49" "$TMPDIR/b.out"

sleep 1
if [ ! -e "$TMPDIR/imux-$(id -u)/default" ]; then echo "ok   server exits when the last pane exits"; else echo "FAIL server still running"; fail=1; fi

grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
