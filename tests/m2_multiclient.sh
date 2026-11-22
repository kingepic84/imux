#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-multi.XXXXXX)
export IMUX_CONFIG=/dev/null IMUX_STATE_DIR="$TMPDIR/state"
fail=0

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

(sleep 1; sleep 3; printf 'echo from-a-$((3*3))\r'; sleep 1; printf 'echo size-$(stty size | tr " " x)\r'; sleep 3; printf '\002d'; sleep 1) \
  | timeout 30 script -q "$TMPDIR/a.out" sh -c "stty rows 30 cols 100; $BIN" >/dev/null &
PID_A=$!

sleep 2

(sleep 3; printf 'echo from-b-$((4*4))\r'; sleep 3; printf '\002d'; sleep 1) \
  | timeout 30 script -q "$TMPDIR/b.out" sh -c "stty rows 12 cols 40; $BIN attach" >/dev/null &
PID_B=$!

wait $PID_A $PID_B

check "A sees its own command output" "from-a-9" "$TMPDIR/a.out"
check "B sees A's command output" "from-a-9" "$TMPDIR/b.out"
check "A sees B's command output" "from-b-16" "$TMPDIR/a.out"
check "B sees its own command output" "from-b-16" "$TMPDIR/b.out"
check "pty uses the smaller client's size minus the status row (A, 30x100 vs B, 12x40)" "size-11x40" "$TMPDIR/a.out"
check "same size output reached B" "size-11x40" "$TMPDIR/b.out"

"$BIN" kill-server >/dev/null 2>&1
rm -rf "$TMPDIR"
exit $fail
