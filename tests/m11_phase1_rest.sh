#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-rest.XXXXXX)
export IMUX_CONFIG=/dev/null
export IMUX_STATE_DIR="$TMPDIR/state"
fail=0

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aq "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 60 script -q "$out" sh -c "stty rows 30 cols 100; $BIN new -s alpha"
}

side_client() {
  local out=$1 keys=$2 args=$3
  (sleep 1; printf '%b' "$keys"; sleep 1) | timeout 20 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $args"
}

(sleep 3; side_client "$TMPDIR/e1.out" '' 'attach -t nosuch') &
(sleep 3; side_client "$TMPDIR/e2.out" '' 'new -s alpha') &
(sleep 3; side_client "$TMPDIR/att.out" '\002d' 'attach -t alpha') &
(sleep 60; $BIN kill-server >/dev/null 2>&1) &
killer=$!

run_client "$TMPDIR/a.out" \
  'export W=1\r' \
  '\002c' \
  'export W=2\r' \
  '\002c' \
  'export W=3\r' \
  '$BIN ls >$TMPDIR/ls1.out\r' \
  '\002:' \
  'swap-window -t 0\r' \
  '\002' '2' \
  'echo at3-$W >>$TMPDIR/pos.out\r' \
  '\002' '0' \
  'echo at1-$W >>$TMPDIR/pos.out\r' \
  '\002:' \
  'move-window -t 2\r' \
  'echo mv-$W >>$TMPDIR/pos.out\r' \
  '\002' '0' \
  'echo p1-$W >>$TMPDIR/pos.out\r' \
  '\002:' \
  'swap-window -t 9\r' \
  '\002:' \
  'rename-session histA\r' \
  '\002:' \
  'rename-session histB\r' \
  '\002:' \
  '\033[A' \
  '\033[A' \
  '\r' \
  '$BIN ls >$TMPDIR/ls2.out\r' \
  '\002:' \
  'new-session\r' \
  '\002:' \
  'kill-session -t nosuch\r' \
  '\002:' \
  'kill-session -t histA\r' \
  '$BIN ls >$TMPDIR/ls3.out\r' \
  '\002:' \
  'kill-session\r'

kill "$killer" 2>/dev/null

check "swap-window -t 1 put the first window last" "^at3-1" "$TMPDIR/pos.out"
check "swap-window -t 1 put the last window first" "^at1-3" "$TMPDIR/pos.out"
check "move-window -t 3 keeps the moved window selected" "^mv-3" "$TMPDIR/pos.out"
check "move-window shifted the others" "^p1-2" "$TMPDIR/pos.out"
check "swap-window to a missing window reports it" "can't find window" "$TMPDIR/a.out"
check "ls shows windows and the attached mark" "alpha: 3 windows (attached)" "$TMPDIR/ls1.out"
check "up arrow at the prompt recalls older commands" "histA: 3 windows" "$TMPDIR/ls2.out"
refute "the recalled line replaced the newer one" "histB" "$TMPDIR/ls2.out"
check "new-session adds a numbered session" "^0: 1 window" "$TMPDIR/ls3.out"
check "kill-session -t unknown reports it" "can't find session" "$TMPDIR/a.out"
refute "kill-session -t removes that session" "histA" "$TMPDIR/ls3.out"
check "attach -t unknown is an error" "can't find session: nosuch" "$TMPDIR/e1.out"
check "new -s with a used name is an error" "duplicate session: alpha" "$TMPDIR/e2.out"
check "attach -t joins the named session" "\[detached\]" "$TMPDIR/att.out"

sleep 1
if [ ! -e "$TMPDIR/imux-$(id -u)/default" ]; then echo "ok   killing the last session ends the server"; else echo "FAIL server still running"; fail=1; fi
if $BIN ls 2>&1 | grep -q "no server running"; then echo "ok   ls without a server says so"; else echo "FAIL ls without a server"; fail=1; fi
if $BIN nonsense | grep -q "usage: imux \[new"; then echo "ok   usage lists the new commands"; else echo "FAIL usage text"; fail=1; fi

grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" | grep -v "can't find session\|duplicate session" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
