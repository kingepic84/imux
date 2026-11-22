#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-snap.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 1
set restore-commands "sleep"
EOF

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aqF -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

cleanup_processes() {
  pkill -f "sleep 31341" 2>/dev/null
  pkill -f "SECRETARG123" 2>/dev/null
  return 0
}

(sleep 100; $BIN kill-server >/dev/null 2>&1; cleanup_processes) &

echo "-- arguments are saved only for programs that are restored"
ARGS="new -s w" HOLD=6 client "$TMPDIR/a.out" \
  "sleep 31341\r" '\002c' "bash -c 'sleep 60; :' SECRETARG123\r" >/dev/null &
CLIENT=$!
sleep 6
$BIN save snap1 >"$TMPDIR/save.out" 2>&1
check "save reports what it wrote" "saved 1 session as snap1" "$TMPDIR/save.out"
check "a restored program keeps its arguments" "sleep 31341" "$IMUX_STATE_DIR/snapshots/snap1.imux"
check "a program that is not restored is recorded by name" " 0 1 bash" "$IMUX_STATE_DIR/snapshots/snap1.imux"
refute "its arguments are not in the named snapshot" "SECRETARG123" "$IMUX_STATE_DIR/snapshots/snap1.imux"
sleep 2
refute "nor in the autosave" "SECRETARG123" "$IMUX_STATE_DIR/autosave.imux"
check "the autosave has the restored program" "31341" "$IMUX_STATE_DIR/autosave.imux"
wait $CLIENT

$BIN kill-server >/dev/null 2>&1
sleep 1
cleanup_processes
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
