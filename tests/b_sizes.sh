#!/bin/bash
# Panes follow the smallest client that looks at their own session, not the smallest client of the
# whole server: a 100-column client on session a is not squeezed by a 60-column client on b.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-sizes.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

until_true() {
  local n=0
  while [ "$n" -lt "$2" ]; do
    eval "$1" && return 0
    sleep 0.5
    n=$((n + 1))
  done
  return 1
}

width_of() {
  $BIN list-panes -a -F '#{session_name} #{pane_width}' 2>/dev/null | awk -v s="$1" '$1 == s {print $2; exit}'
}

timed_client() {
  local out=$1 rows=$2 cols=$3 args=$4
  shift 4
  (for step in "$@"; do sleep "${step%%:*}"; printf '%b' "${step#*:}"; done; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows $rows cols $cols; $BIN $args" >/dev/null
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

timed_client "$TMPDIR/a.out" 30 100 "new -s a" "1:" "14:\002)" "60:\002d" &
CLIENTA=$!
sleep 2
timed_client "$TMPDIR/b.out" 20 60 "new -s b" "1:" "24:\002d" &
CLIENTB=$!

until_true '[ -n "$(width_of a)" ] && [ -n "$(width_of b)" ]' 30
assert "session a keeps the width of its own client" '[ "$(width_of a)" = 100 ]'
assert "session b follows the narrower client that looks at it" '[ "$(width_of b)" = 60 ]'

$BIN new-session -d -s d >/dev/null 2>&1
assert "a session nobody looks at stays 80 columns" 'until_true "[ \"\$(width_of d)\" = 80 ]" 6'

echo "-- client a switches to session b"
assert "client a moved to session b" 'until_true "[ \"\$($BIN list-clients -F \"#{client_session}\" 2>/dev/null | grep -c \"^b\$\")\" = 2 ]" 50'
assert "both clients on b: the narrower one decides" '[ "$(width_of b)" = 60 ]'
assert "session a keeps its last size when nobody looks at it" '[ "$(width_of a)" = 100 ]'
assert "session d is untouched by the switch" '[ "$(width_of d)" = 80 ]'

echo "-- the narrow client leaves"
wait $CLIENTB
assert "b grows to the remaining client" 'until_true "[ \"\$(width_of b)\" = 100 ]" 20'

$BIN kill-server >/dev/null 2>&1
wait $CLIENTA 2>/dev/null
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
