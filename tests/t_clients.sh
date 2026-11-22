#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-clients.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
printf 'set clock-mode-color colour196\n' >"$IMUX_CONFIG"
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

(sleep 240; $BIN kill-server >/dev/null 2>&1) &

echo "-- clock-mode"
ARGS="new -s w" HOLD=1 client "$TMPDIR/a.out" \
  '\002t' 'x' \
  'echo clk-$((1+2))\r' >/dev/null
check "the clock is drawn in the color of clock-mode-color" '48;5;196' "$TMPDIR/a.out"
check "a key leaves the clock and is not typed into the pane" 'clk-3' "$TMPDIR/a.out"
assert "the key that left the clock did not reach the shell" '! grep -aq "xecho clk" "$TMPDIR/a.out"'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- mark pane and swap-pane"
ARGS="new -s w" HOLD=1 client "$TMPDIR/b.out" \
  '\002m' \
  '\002%' \
  'echo right-$(stty size | tr " " x)\r' \
  '\002:' 'swap-pane\r' \
  'echo swapped-$(stty size | tr " " x)\r' \
  '\002M' >/dev/null
check "m marks the pane" 'marked pane %0' "$TMPDIR/b.out"
check "the new pane is on the right with 49 columns" 'right-29x49' "$TMPDIR/b.out"
check "swap-pane puts the active pane where the marked one was" 'swapped-29x50' "$TMPDIR/b.out"
check "M clears the mark" 'mark cleared' "$TMPDIR/b.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- swap-pane without a mark"
ARGS="new -s w" HOLD=1 client "$TMPDIR/c.out" \
  '\002%' \
  '\002:' 'swap-pane\r' >/dev/null
check "it says there is no marked pane" 'no marked pane' "$TMPDIR/c.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- choose-client"
ARGS="new -s w" HOLD=1 client "$TMPDIR/d.out" \
  '\002D' >/dev/null
check "the list shows the client with its size and session, and marks the one asking" 'client [0-9]+: 100x30 \[w\] \*' "$TMPDIR/d.out"
$BIN kill-server >/dev/null 2>&1
sleep 1
ARGS="new -s w" HOLD=1 client "$TMPDIR/e.out" \
  '\002D' '\r' >/dev/null
check "choosing a client detaches it" '\[detached\]' "$TMPDIR/e.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- suspend-client"
(sleep 1; printf '%b' 'echo before-s\r'; sleep 0.8; printf '%b' '\002\x1a'; sleep 6; printf '%b' 'echo after-$((1+2))\r'; sleep 1.5; printf '\002d'; sleep 1) \
  | timeout 60 script -q "$TMPDIR/f.out" sh -c "stty rows 30 cols 100; $BIN new -s susp" >/dev/null &
WAIT=$!
stopped=no
for _ in $(seq 1 40); do
  sleep 0.25
  for pid in $(pgrep -f "imux new -s susp"); do
    if ps -o stat= -p "$pid" | grep -q T; then
      stopped=yes
      kill -CONT "$pid"
    fi
  done
  [ "$stopped" = yes ] && break
done
assert "the client stops like a job when it is told to suspend" '[ "$stopped" = yes ]'
wait $WAIT
check "after it continues the screen is drawn again and works" 'after-3' "$TMPDIR/f.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
