#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-smoke.XXXXXX)
export IMUX_CONFIG=/dev/null IMUX_STATE_DIR="$TMPDIR/state"
fail=0

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 20 script -q "$out" sh -c "stty rows 24 cols 80; $BIN $CLIENT_ARGS"
}

CLIENT_ARGS="" run_client "$TMPDIR/a.out" 'export FOO=persist-$((20+1))\r' 'echo first-$FOO\r' '\002d'
check "first client sees command output" "first-persist-21" "$TMPDIR/a.out"
check "detach message" "\[detached\]" "$TMPDIR/a.out"

if [ -S "$TMPDIR/imux-$(id -u)/default" ]; then echo "ok   server still running after detach"; else echo "FAIL server gone after detach"; fail=1; fi

CLIENT_ARGS="attach" run_client "$TMPDIR/b.out" 'echo second-$FOO\r' 'exit\r'
check "reattached shell kept its environment" "second-persist-21" "$TMPDIR/b.out"

sleep 1
if [ ! -e "$TMPDIR/imux-$(id -u)/default" ]; then echo "ok   socket removed after shell exit"; else echo "FAIL socket left behind"; fail=1; fi

CLIENT_ARGS="attach" run_client "$TMPDIR/c.out" 'x\r'
check "attach with no server fails cleanly" "no server running" "$TMPDIR/c.out"

grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
