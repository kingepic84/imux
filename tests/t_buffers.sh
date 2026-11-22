#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-buffers.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
printf 'set copy-command ""\nset set-clipboard off\n' >"$IMUX_CONFIG"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) &

echo "-- set-buffer, paste-buffer, list-buffers, delete-buffer"
ARGS="new -s w" HOLD=1 client "$TMPDIR/a.out" \
  '\002:' $'set-buffer "\'x\'y"\r' \
  'echo ok-' '\002]' '\r' \
  '\002:' 'set-buffer -b mine hello\r' \
  '\002#' '\033' \
  '\002-' \
  '\002#' '\033' \
  '\002:' 'paste-buffer -b mine\r' >/dev/null
check "a pasted buffer is typed into the pane (the shell strips the quotes)" 'ok-xy' "$TMPDIR/a.out"
check "list-buffers shows an automatic buffer with its size" "buffer0: 4 bytes: \"'x'y\"" "$TMPDIR/a.out"
check "and a named one, newest first" 'mine: 5 bytes: "hello"' "$TMPDIR/a.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- delete-buffer takes the newest, a named paste takes its own"
ARGS="new -s w" HOLD=1 client "$TMPDIR/b.out" \
  '\002:' 'set-buffer -b first one-text\r' \
  '\002:' 'set-buffer -b second two-text\r' \
  '\002-' \
  'echo gone-' '\002:' 'paste-buffer -b first\r' '\r' \
  '\002-' \
  '\002-' \
  '\002#' >/dev/null
check "paste-buffer -b pastes that buffer" 'gone-one-text' "$TMPDIR/b.out"
check "a delete on an empty stack says so" 'no buffers' "$TMPDIR/b.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- choose-buffer"
ARGS="new -s w" HOLD=1 client "$TMPDIR/c.out" \
  '\002:' 'set-buffer -b lower low-part\r' \
  '\002:' 'set-buffer -b upper up-part\r' \
  '\002=' '\033' \
  'echo pick-' '\002=' 'j' '\r' '\r' >/dev/null
check "the chooser lists the buffers" 'lower: 8 bytes' "$TMPDIR/c.out"
check "choosing one pastes it" 'pick-low-part' "$TMPDIR/c.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- escapes in a buffer never reach the pane, save and load"
printf 'a\033b\n' >"$TMPDIR/esc.txt"
ARGS="new -s w" HOLD=1 client "$TMPDIR/d.out" \
  '\002:' "load-buffer $TMPDIR/esc.txt\\r" \
  'echo e-' '\002]' '\r' \
  '\002:' "save-buffer $TMPDIR/saved.txt\\r" \
  '\002:' "save-buffer -a $TMPDIR/saved.txt\\r" >/dev/null
check "the escape is gone and the rest is typed" 'e-ab' "$TMPDIR/d.out"
assert "save-buffer writes the buffer as it is, and -a adds to the file" '[ "$(wc -c <"$TMPDIR/saved.txt")" -eq 8 ]'
assert "with the escape kept in the file (it is the paste that cleans)" 'grep -aqF "$(printf "a\033b")" "$TMPDIR/saved.txt"'
assert "and the file is private" '[ "$(stat -f %Lp "$TMPDIR/saved.txt")" = 600 ]'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- buffer-limit"
ARGS="new -s w" HOLD=1 client "$TMPDIR/e.out" \
  '\002:' 'set buffer-limit 2\r' \
  '\002:' 'set-buffer one\r' \
  '\002:' 'set-buffer two\r' \
  '\002:' 'set-buffer three\r' \
  '\002#' >/dev/null
check "the newest are kept" 'buffer2: 5 bytes: "three"' "$TMPDIR/e.out"
check "and the one before" 'buffer1: 3 bytes: "two"' "$TMPDIR/e.out"
assert "the oldest is dropped" '! grep -aq "buffer0: 3 bytes" "$TMPDIR/e.out"'
$BIN kill-server >/dev/null 2>&1
sleep 1

grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
