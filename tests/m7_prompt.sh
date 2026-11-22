#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-prompt.XXXXXX)
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

(sleep 12; $BIN kill-server >/dev/null 2>&1) &
run_client "$TMPDIR/a.out" \
  '\002:' \
  'split-window -h\r' \
  'echo split-size-$(stty size | tr " " x)\r' \
  '\002:' \
  'frobnicate\r' \
  '\002q' \
  '0' \
  'echo picked-size-$(stty size | tr " " x)\r' \
  '\002:' \
  'resize-pane -R 4\r' \
  'echo resized-size-$(stty size | tr " " x)\r' \
  '\002:' \
  'rename-window "my build"\r' \
  '\002:' \
  'new-window\r' \
  '\002p'

check "split-window -h at the prompt splits side by side" "split-size-29x49" "$TMPDIR/a.out"
check "an unknown command shows a message" "unknown command: frobnicate" "$TMPDIR/a.out"
check "pane numbers: pressing 0 focuses the first pane" "picked-size-29x50" "$TMPDIR/a.out"
check "resize-pane -R at the prompt" "resized-size-29x54" "$TMPDIR/a.out"
check "rename-window with a quoted name" "my build" "$TMPDIR/a.out"
check "new-window at the prompt adds a tab" "1:[a-z]" "$TMPDIR/a.out"

sleep 1
grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
