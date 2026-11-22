#!/bin/bash
# The protocol version check: a client and a server of different versions say so instead of
# misbehaving, `kill-server` and `doctor` work either way, and an older server (one that hangs up
# on the version message) is recognized. A debug build lets IMUX_TEST_PROTO_VERSION change the
# version a process speaks.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
HERE="$(cd "$(dirname "$0")" && pwd)"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qv.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

echo "-- the same version"
$BIN new-session -d -s v >/dev/null 2>&1
$BIN ls >"$TMPDIR/o1" 2>&1
assert "a request works" '[ $? -eq 0 ] && grep -aq "^v" "$TMPDIR/o1"'
$BIN doctor >"$TMPDIR/d1" 2>&1
assert "doctor says the server speaks the same version" 'grep -aqE "^protocol: [0-9]+ \(the server speaks it too\)$" "$TMPDIR/d1"'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- a server of another version"
IMUX_TEST_PROTO_VERSION=99 $BIN new-session -d -s w >"$TMPDIR/o2" 2>&1
assert "its own kind of client is served" '[ $? -eq 0 ]'
IMUX_TEST_PROTO_VERSION=99 $BIN ls >"$TMPDIR/o3" 2>&1
assert "a client of its version works" 'grep -aq "^w" "$TMPDIR/o3"'
start=$(date +%s)
$BIN ls >"$TMPDIR/o4" 2>&1
rc=$?
end=$(date +%s)
assert "a client of another version is refused" '[ $rc -ne 0 ]'
assert "with both versions named" 'grep -aq "protocol 2 but the server speaks 99" "$TMPDIR/o4"'
assert "the way out and the server's pid" 'grep -aq "imux kill-server" "$TMPDIR/o4" && grep -aqE "kill [0-9]+" "$TMPDIR/o4"'
assert "and quickly" '[ $((end - start)) -le 3 ]'
$BIN list-windows >"$TMPDIR/o5" 2>&1
assert "a command is refused too" '[ $? -ne 0 ] && grep -aq "server speaks 99" "$TMPDIR/o5"'
$BIN events >"$TMPDIR/o6" 2>&1
assert "events is refused" '[ $? -ne 0 ] && grep -aq "server speaks 99" "$TMPDIR/o6"'
(sleep 2; printf '\002d'; sleep 0.5) | timeout 20 script -q "$TMPDIR/c1.out" sh -c "stty rows 30 cols 100; $BIN attach -t w" >/dev/null 2>&1
assert "attach shows the refusal and leaves the terminal alone" 'grep -aq "error: .*server speaks 99" "$TMPDIR/c1.out"'
$BIN doctor >"$TMPDIR/d2" 2>&1
assert "doctor reports the mismatch as a problem" 'grep -aq "^protocol: 2 PROBLEM: this imux speaks protocol 2 but the server speaks 99" "$TMPDIR/d2"'
assert "and still says a server runs" 'grep -aq "^server: running" "$TMPDIR/d2"'

echo "-- kill-server works against any version"
$BIN kill-server >"$TMPDIR/o7" 2>&1
assert "it is not refused" '[ $? -eq 0 ] && ! grep -aq "protocol" "$TMPDIR/o7"'
sleep 1
$BIN ls >"$TMPDIR/o8" 2>&1
assert "and the server is gone" 'grep -aq "no server running" "$TMPDIR/o8"'
$BIN new-session -d -s again >/dev/null 2>&1
$BIN ls >"$TMPDIR/o9" 2>&1
assert "a new one at this version works" 'grep -aq "^again" "$TMPDIR/o9"'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- an older server"
if command -v python3 >/dev/null 2>&1; then
  mkdir -p -m 700 "$TMPDIR/imux-$(id -u)"
  SOCK="$TMPDIR/imux-$(id -u)/default"
  python3 -I "$HERE/fixtures/old_server.py" "$SOCK" 4 >"$TMPDIR/fake.out" 2>&1 &
  FAKE=$!
  for i in $(seq 1 20); do grep -aq ready "$TMPDIR/fake.out" 2>/dev/null && break; sleep 0.25; done
  $BIN ls >"$TMPDIR/p1" 2>&1
  assert "a hang-up on the version is taken for an older server" '[ $? -ne 0 ] && grep -aq "older imux" "$TMPDIR/p1" && grep -aq "imux kill-server" "$TMPDIR/p1"'
  (sleep 1; printf '\002d') | timeout 20 script -q "$TMPDIR/c2.out" sh -c "stty rows 30 cols 100; $BIN attach -t x" >/dev/null 2>&1
  assert "so does attach" 'grep -aq "older imux" "$TMPDIR/c2.out"'
  $BIN doctor >"$TMPDIR/d3" 2>&1
  assert "doctor says the server predates the check" 'grep -aq "^protocol: .*PROBLEM: the server is older than the version check" "$TMPDIR/d3"'
  kill $FAKE >/dev/null 2>&1
  rm -f "$SOCK"
else
  echo "skip no python3: the older-server checks"
fi

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" | grep -av "protocol\|older" && { echo "FAIL unexpected errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
