#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-status.XXXXXX)
fail=0
export IMUX_CONFIG="$(cd "$(dirname "$0")" && pwd)/fixtures/example.imux.conf" IMUX_STATE_DIR="$TMPDIR/state"

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 30 script -q "$out" sh -c "stty rows 30 cols 120; $BIN"
}

(sleep 7; $BIN kill-server >/dev/null 2>&1) &
run_client "$TMPDIR/a.out" \
  '\001c' \
  '\001:' \
  'display "hello msg"\r' \
  '\0011'

check "example config: current tab text" " 1  zsh " "$TMPDIR/a.out"
check "example config: second tab text" " 2  zsh " "$TMPDIR/a.out"
check "example config: status-left shows the session" "󰌪 0 " "$TMPDIR/a.out"
check "example config: clock shows AM or PM" "[AP]M" "$TMPDIR/a.out"
check "example config: display message" "hello msg" "$TMPDIR/a.out"
check "example config: current tab is bold on blue" "1;38;2;0;0;0;48;2;137;180;250" "$TMPDIR/a.out"

sleep 1
LOG="$TMPDIR/imux-$(id -u)/server.log"
grep -a '"level":"warn".*config: ' "$LOG" && { echo "FAIL config errors in server.log"; fail=1; }
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
