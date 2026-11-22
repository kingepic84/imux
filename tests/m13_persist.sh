#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-persist.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
SOCK="$TMPDIR/imux-$(id -u)/default"
LOG="$TMPDIR/imux-$(id -u)/server.log"

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 1
set restore-commands "sleep"
EOF

mkdir -p "$TMPDIR/d1" "$TMPDIR/d2" "$TMPDIR/d3"
P1=$(cd "$TMPDIR/d1" && pwd -P)
P2=$(cd "$TMPDIR/d2" && pwd -P)
P3=$(cd "$TMPDIR/d3" && pwd -P)

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aqF -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

server_pid() {
  local p
  for p in $(lsof -t "$SOCK" 2>/dev/null); do
    if ps -o command= -p "$p" | grep -q ' server$'; then echo "$p"; return; fi
  done
}

cleanup_processes() {
  pkill -f "sleep 31337" 2>/dev/null
  pkill -f "tail -F $TMPDIR/na" 2>/dev/null
  return 0
}

(sleep 150; $BIN kill-server >/dev/null 2>&1; cleanup_processes) &

echo "-- build a session, save it by name"
ARGS="new -s work" HOLD=8 client "$TMPDIR/a.out" \
  "cd $P1\r" '\002%' "cd $P2\r" '\002c' "cd $P3\r" 'sleep 31337\r' '\002c' "tail -F $TMPDIR/na\r" >/dev/null &
CLIENT=$!
sleep 7
$BIN save snap1 >"$TMPDIR/save.out" 2>&1
$BIN snapshots >"$TMPDIR/snaps.out" 2>&1
check "save reports what it wrote" "saved 1 session as snap1" "$TMPDIR/save.out"
check "snapshots lists the saved name" "snap1" "$TMPDIR/snaps.out"
check "the file records the first directory" "$P1" "$IMUX_STATE_DIR/snapshots/snap1.imux"
check "the file records the second directory" "$P2" "$IMUX_STATE_DIR/snapshots/snap1.imux"
check "the file records the running command" "sleep 31337" "$IMUX_STATE_DIR/snapshots/snap1.imux"
assert "snapshot files are private" '[ "$(stat -f %Lp "$IMUX_STATE_DIR/snapshots/snap1.imux")" = 600 ]'
assert "the state directory is private" '[ "$(stat -f %Lp "$IMUX_STATE_DIR")" = 700 ]'
wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1
cleanup_processes
assert "kill-server removes the autosave" '[ ! -e "$IMUX_STATE_DIR/autosave.imux" ]'

echo "-- restore the snapshot into a fresh server"
ARGS="new -s other" HOLD=12 client "$TMPDIR/b.out" >/dev/null &
CLIENT=$!
sleep 3
$BIN restore snap1 >"$TMPDIR/restore.out" 2>&1
$BIN restore snap1 >"$TMPDIR/restore2.out" 2>&1
$BIN ls >"$TMPDIR/ls.out" 2>&1
check "restore reports the session" "restored 1 session" "$TMPDIR/restore.out"
check "restoring again leaves the existing name alone" "left alone" "$TMPDIR/restore2.out"
check "the restored session has its three windows" "work: 3 windows" "$TMPDIR/ls.out"
sleep 4
assert "an allowed command runs again" 'pgrep -f "sleep 31337" >/dev/null'
assert "a command that is not allowed does not" '! pgrep -f "tail -F $TMPDIR/na" >/dev/null'
wait $CLIENT
ARGS="attach -t work" HOLD=1 client "$TMPDIR/c.out" \
  '\002' '0' 'echo cwd-$(pwd -P)-a\r' '\002o' 'echo cwd-$(pwd -P)-b\r' >/dev/null
check "the active pane of a window is the one that was active" "cwd-$P2-a" "$TMPDIR/c.out"
check "the other pane of the window kept its directory" "cwd-$P1-b" "$TMPDIR/c.out"

echo "-- autosave, SIGTERM keeps it"
sleep 3
assert "an autosave appears by itself" '[ -s "$IMUX_STATE_DIR/autosave.imux" ]'
assert "autosave files are private" '[ "$(stat -f %Lp "$IMUX_STATE_DIR/autosave.imux")" = 600 ]'
SPID=$(server_pid)
assert "found the server process" '[ -n "$SPID" ]'
kill -TERM "$SPID"
sleep 2
cleanup_processes
assert "the server is gone after SIGTERM" '[ ! -e "$SOCK" ]'
assert "SIGTERM leaves the autosave" '[ -s "$IMUX_STATE_DIR/autosave.imux" ]'
check "the autosave holds the session" "work" "$IMUX_STATE_DIR/autosave.imux"

echo "-- the next start brings it back"
ARGS="" HOLD=6 client "$TMPDIR/d.out" >/dev/null &
CLIENT=$!
sleep 3
$BIN ls >"$TMPDIR/ls2.out" 2>&1
check "starting without arguments restores the autosave" "work: 3 windows" "$TMPDIR/ls2.out"
check "and the other session with it" "other: 1 window" "$TMPDIR/ls2.out"
wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1
cleanup_processes
assert "kill-server removes both autosave files" '[ ! -e "$IMUX_STATE_DIR/autosave.imux" ] && [ ! -e "$IMUX_STATE_DIR/autosave.prev" ]'
assert "kill-server keeps named snapshots" '[ -s "$IMUX_STATE_DIR/snapshots/snap1.imux" ]'

echo "-- a damaged autosave is a clean start"
printf 'imux-snapshot 1\nsaved 1\nrec' >"$IMUX_STATE_DIR/autosave.imux"
printf 'garbage\n' >"$IMUX_STATE_DIR/autosave.prev"
ARGS="new -s fresh" HOLD=4 client "$TMPDIR/e.out" >/dev/null &
CLIENT=$!
sleep 3
$BIN ls >"$TMPDIR/ls3.out" 2>&1
check "a damaged autosave starts the requested session" "fresh: 1 window" "$TMPDIR/ls3.out"
refute "and restores nothing from it" "work" "$TMPDIR/ls3.out"
check "the damage is logged" "autosave.imux unusable" "$LOG"
wait $CLIENT

echo "-- delete"
$BIN delete-snapshot snap1 >"$TMPDIR/del.out" 2>&1
check "delete-snapshot reports it" "deleted snap1" "$TMPDIR/del.out"
$BIN delete-snapshot snap1 >"$TMPDIR/del2.out" 2>&1
check "deleting a missing snapshot is an error" "no snapshot named snap1" "$TMPDIR/del2.out"
$BIN save "../evil" >"$TMPDIR/evil.out" 2>&1
check "a name that is a path is refused" "bad snapshot name" "$TMPDIR/evil.out"
assert "nothing was written outside the store" '[ ! -e "$IMUX_STATE_DIR/evil.imux" ] && [ ! -e "$TMPDIR/evil.imux" ]'

$BIN kill-server >/dev/null 2>&1
sleep 1
cleanup_processes
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
