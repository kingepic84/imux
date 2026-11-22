#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-input.XXXXXX)
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

(sleep 11; $BIN kill-server >/dev/null 2>&1) &
run_client "$TMPDIR/a.out" \
  '\033[98;5u' \
  '%' \
  'echo csiu-size-$(stty size | tr " " x)\r' \
  '\002\033[1;3D\033[1;3D' \
  'echo repeat-size-$(stty size | tr " " x)\r' \
  '\002y' \
  'echo unbound-$((2*21))\r' \
  '\033' \
  '\002"' \
  'echo escape-$(stty size | tr " " x)\r' \
  'echo done\r'

check "CSI u encoded prefix is recognized" "csiu-size-29x49" "$TMPDIR/a.out"
check "a repeatable key pressed again within repeat-time needs no prefix" "repeat-size-29x59" "$TMPDIR/a.out"
check "an unbound key after the prefix is dropped" "unbound-42" "$TMPDIR/a.out"
check "a lone escape is not glued to the next key" "escape-14x59" "$TMPDIR/a.out"

sleep 1

grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
