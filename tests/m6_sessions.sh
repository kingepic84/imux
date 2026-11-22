#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-sessions.XXXXXX)
export IMUX_CONFIG=/dev/null IMUX_STATE_DIR="$TMPDIR/state"
fail=0

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 40 script -q "$out" sh -c "stty rows 30 cols 100; $BIN"
}

run_client "$TMPDIR/a.out" \
  'export S=one\r' \
  '\002N' \
  'export S=two\r' \
  'echo ses-$S\r' \
  '\002(' \
  'echo prev-$S\r' \
  '\002)' \
  'echo next-$S\r' \
  '\002s' \
  'k' \
  '\r' \
  'echo picked-$S\r' \
  '\002,' \
  '\025renamed\r' \
  'echo after-rename\r' \
  '\002w' \
  '\033' \
  'echo esc-ok-$((3+4))\r' \
  '\033[<0;2;30M\033[<0;2;30m' \
  'j' \
  '\r' \
  'echo clicked-$S\r' \
  '\002c' \
  'export W=b\r' \
  '\002' '0' \
  'echo first-win\r' \
  '\002\t' \
  'echo last-$W\r' \
  'exit\r' \
  'exit\r' \
  'echo home-$S\r' \
  'exit\r' \
  'exit\r'

check "new session starts a fresh shell" "ses-two" "$TMPDIR/a.out"
check "prefix ( goes to the previous session" "prev-one" "$TMPDIR/a.out"
check "prefix ) goes to the next session" "next-two" "$TMPDIR/a.out"
check "session picker draws its title" "sessions" "$TMPDIR/a.out"
check "picker lists sessions with window counts" "1 window" "$TMPDIR/a.out"
check "picker selection switches session on Enter" "picked-one" "$TMPDIR/a.out"
check "rename prompt shows its label" "rename-window" "$TMPDIR/a.out"
check "renamed window shows in its tab" "0:renamed" "$TMPDIR/a.out"
check "window picker draws its title" "sessions and windows" "$TMPDIR/a.out"
check "escape closes the picker and keys reach the pane again" "esc-ok-7" "$TMPDIR/a.out"
check "clicking the session segment opens the picker" "clicked-two" "$TMPDIR/a.out"
check "prefix tab returns to the last window" "last-b" "$TMPDIR/a.out"
check "closing a session's last pane moves the client to another session" "home-one" "$TMPDIR/a.out"

sleep 1
if [ ! -e "$TMPDIR/imux-$(id -u)/default" ]; then echo "ok   server exits when the last session ends"; else echo "FAIL server still running"; fail=1; fi

grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
