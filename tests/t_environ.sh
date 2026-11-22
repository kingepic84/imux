#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-environ.XXXXXX)
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

export SSH_AUTH_SOCK=/sock/a
export TESTVAR=from-the-server
ARGS="new -s w" HOLD=40 client "$TMPDIR/a.out" 'echo up\r' >/dev/null &
CLIENTA=$!
sleep 3

$BIN show-environment SSH_AUTH_SOCK >"$TMPDIR/o1" 2>&1
check "the first client's SSH_AUTH_SOCK is kept for new panes" '^SSH_AUTH_SOCK=/sock/a$' "$TMPDIR/o1"

(
  export SSH_AUTH_SOCK=/sock/b
  ARGS="attach -t w" HOLD=6 client "$TMPDIR/b.out" >/dev/null
) &
CLIENTB=$!
sleep 4
$BIN show-environment SSH_AUTH_SOCK >"$TMPDIR/o2" 2>&1
check "a client that attaches later updates it" '^SSH_AUTH_SOCK=/sock/b$' "$TMPDIR/o2"
$BIN new-window -t w -n e >/dev/null 2>&1
sleep 1
$BIN send-keys -t w:e 'echo env-$SSH_AUTH_SOCK-' Enter >/dev/null 2>&1
sleep 1
$BIN capture-pane -p -t w:e >"$TMPDIR/o3" 2>&1
check "and a pane made afterwards has the new value" '^env-/sock/b-$' "$TMPDIR/o3"

echo "-- set-environment"
$BIN set-environment MYVAR hello >/dev/null 2>&1
$BIN set-environment -u TESTVAR >/dev/null 2>&1
$BIN show-environment >"$TMPDIR/o4" 2>&1
check "set-environment shows in show-environment" '^MYVAR=hello$' "$TMPDIR/o4"
check "-u shows as -NAME" '^-TESTVAR$' "$TMPDIR/o4"
$BIN new-window -t w -n f >/dev/null 2>&1
sleep 1
$BIN send-keys -t w:f 'echo my-$MYVAR-${TESTVAR:-gone}-' Enter >/dev/null 2>&1
sleep 1
$BIN capture-pane -p -t w:f >"$TMPDIR/o5" 2>&1
check "new panes get the variable and lose the unset one" '^my-hello-gone-$' "$TMPDIR/o5"
$BIN set-environment -r TESTVAR >/dev/null 2>&1
$BIN show-environment TESTVAR >"$TMPDIR/o6" 2>&1
check "-r forgets the entry, the server's own value shows again" '^TESTVAR=from-the-server$' "$TMPDIR/o6"
$BIN set-environment 'A=B' x >"$TMPDIR/o7" 2>&1
assert "a bad name is an error" '[ $? -ne 0 ] && grep -aq "bad variable name" "$TMPDIR/o7"'
$BIN show-environment NOPE_NOT_SET >"$TMPDIR/o8" 2>&1
assert "an unknown variable is an error" '[ $? -ne 0 ] && grep -aq "unknown variable" "$TMPDIR/o8"'

wait $CLIENTB
$BIN kill-server >/dev/null 2>&1
wait $CLIENTA
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
