#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-lookup.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
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

echo "-- list-keys, show-messages, display-message"
ARGS="new -s w" HOLD=1 client "$TMPDIR/a.out" \
  '\002?' '\033' \
  '\002:' 'display-message "hello-msg-#S"\r' \
  '\002~' '\033' \
  '\002i' >/dev/null
check "? opens the list of keys with a title" 'keys' "$TMPDIR/a.out"
check "which has the split binding" 'bind-key -T prefix % +split-window -h' "$TMPDIR/a.out"
check "display-message fills in variables" 'hello-msg-w' "$TMPDIR/a.out"
check "~ opens the list of messages" 'messages' "$TMPDIR/a.out"
check "which has the message with a time" '[0-9][0-9]:[0-9][0-9]:[0-9][0-9] hello-msg-w' "$TMPDIR/a.out"
check "i shows where the client is" '\[w\] 0:[a-z]+, pane 0' "$TMPDIR/a.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- imux list-keys"
ARGS="new -s w" HOLD=4 client "$TMPDIR/b.out" 'echo up\r' >/dev/null &
CLIENT=$!
sleep 3
$BIN list-keys >"$TMPDIR/keys.txt" 2>&1
check "the subcommand lists the bindings" '^bind-key -T prefix % +split-window -h$' "$TMPDIR/keys.txt"
check "with the repeatable ones marked" '^bind-key -r -T prefix Up +select-pane -U$' "$TMPDIR/keys.txt"
wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- find-window by name and by what a window shows"
ARGS="new -s w" HOLD=1 client "$TMPDIR/c.out" \
  'echo uniquemark-77\r' \
  '\002c' \
  '\002:' 'rename-window zzzname\r' \
  '\002f' 'uniquemark\r' \
  'echo screen-$IMUX_PANE_ID\r' \
  '\002f' 'zzzname\r' \
  'echo name-$IMUX_PANE_ID\r' \
  '\002f' 'nosuchthing\r' >/dev/null
check "a text on a window's screen finds that window" 'screen-0' "$TMPDIR/c.out"
check "a window name finds the window" 'name-1' "$TMPDIR/c.out"
check "no match is said" 'no windows matching: nosuchthing' "$TMPDIR/c.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- the way back to the last session, and window numbers"
ARGS="new -s w" HOLD=1 client "$TMPDIR/d.out" \
  'export S=one\r' \
  '\002N' \
  'export S=two\r' \
  '\002L' \
  'echo session-$S\r' \
  '\002L' \
  'echo session-$S\r' \
  '\002c' \
  "\002'" '0\r' \
  'echo number-$IMUX_PANE_ID\r' \
  '\002r' \
  'echo refreshed\r' >/dev/null
check "L goes back to the last session" 'session-one' "$TMPDIR/d.out"
check "and again goes forward" 'session-two' "$TMPDIR/d.out"
check "' selects a window by number" 'number-[0-9]' "$TMPDIR/d.out"
check "refresh-client keeps the session working" 'refreshed' "$TMPDIR/d.out"

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
