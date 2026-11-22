#!/bin/bash
# The fuzzy filter in pickers (`/` then a query): the session picker, the repository wizard, a text
# list, and the keys that end or clear a query.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qf.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

session_of_client() {
  $BIN list-clients -F '#{client_session}' 2>/dev/null | head -1
}

until_session() {
  local i
  for i in $(seq 1 40); do
    [ "$(session_of_client)" = "$1" ] && return 0
    sleep 0.25
  done
  return 1
}

$BIN new-session -d -s alpha >/dev/null 2>&1
$BIN new-session -d -s beta >/dev/null 2>&1
$BIN new-session -d -s gamma >/dev/null 2>&1

echo "-- the session picker"
(sleep 2; printf '\002s'; sleep 0.7; printf '/'; sleep 0.5; printf 'bet'; sleep 0.7; printf '\r'; sleep 0.7; printf '\r'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c1.out" sh -c "stty rows 30 cols 100; $BIN attach -t alpha" >/dev/null 2>&1 &
CL=$!
assert "typing a query and Enter twice switches to the session that matches" 'until_session beta'
wait $CL
assert "the query and its count are on the box's border" 'grep -aq "/bet_ 1/3" "$TMPDIR/c1.out"'

echo "-- keys of the picker are free until a slash"
(sleep 2; printf '\002s'; sleep 0.7; printf 'j'; sleep 0.5; printf 'j'; sleep 0.5; printf '\r'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c2.out" sh -c "stty rows 30 cols 100; $BIN attach -t alpha" >/dev/null 2>&1 &
CL=$!
assert "j and Enter still move and choose" 'until_session gamma'
wait $CL

echo "-- Escape clears the query, a second Escape closes"
(sleep 2; printf '\002s'; sleep 0.7; printf '/'; sleep 0.5; printf 'gam'; sleep 0.7; printf '\033'; sleep 0.7; printf 'k'; sleep 0.5; printf '\r'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c3.out" sh -c "stty rows 30 cols 100; $BIN attach -t alpha" >/dev/null 2>&1 &
CL=$!
assert "after Escape the whole list is back, still on gamma, and k moves up in it" 'until_session beta'
wait $CL
(sleep 2; printf '\002s'; sleep 0.7; printf '/'; sleep 0.5; printf 'gam'; sleep 0.7; printf '\033'; sleep 0.7; printf '\033'; sleep 0.7; printf '\033'; sleep 0.7; printf '\r'; sleep 1; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c4.out" sh -c "stty rows 30 cols 100; $BIN attach -t alpha" >/dev/null 2>&1 &
CL=$!
sleep 7
assert "Escape after Escape closes the picker, so the client stays where it was" '[ "$(session_of_client)" = alpha ]'
wait $CL

echo "-- no match, then Enter"
(sleep 2; printf '\002s'; sleep 0.7; printf '/'; sleep 0.5; printf 'zzz'; sleep 0.7; printf '\r'; sleep 0.7; printf '\r'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c5.out" sh -c "stty rows 30 cols 100; $BIN attach -t alpha" >/dev/null 2>&1 &
CL=$!
sleep 6
assert "choosing from an empty list closes it and changes nothing" '[ "$(session_of_client)" = alpha ]'
wait $CL

echo "-- the repository wizard"
mkdir -p "$TMPDIR/repos/apple/.git" "$TMPDIR/repos/banana/.git" "$TMPDIR/repos/cherry/.git"
$BIN set -g repos-dir "$TMPDIR/repos" >/dev/null 2>&1
(sleep 2; printf '\002W'; sleep 0.8; printf '/'; sleep 0.5; printf 'ban'; sleep 0.7; printf '\r'; sleep 0.7; printf '\r'; sleep 1.2; printf '\033'; sleep 0.5; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c6.out" sh -c "stty rows 30 cols 100; $BIN attach -t alpha" >/dev/null 2>&1 &
CL=$!
wait $CL
assert "the filtered repository asks for a worktree name" 'grep -aq "worktree name" "$TMPDIR/c6.out"'

echo "-- a text list"
(sleep 2; printf '\002?'; sleep 0.8; printf '/'; sleep 0.5; printf 'detach'; sleep 1; printf '\033'; sleep 0.5; printf '\033'; sleep 0.5; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c7.out" sh -c "stty rows 30 cols 100; $BIN attach -t alpha" >/dev/null 2>&1 &
CL=$!
wait $CL
assert "list-keys can be filtered too" 'grep -aq "/detach_ " "$TMPDIR/c7.out"'

echo "-- the agent panel"
for n in red green; do
  cat >"$TMPDIR/report-$n.sh" <<EOF
#!/bin/sh
$BIN pane report-agent --source hook --agent fake --state working --seq 1 --message 'status $n'
sleep 100
EOF
  chmod +x "$TMPDIR/report-$n.sh"
  $BIN new-session -d -s "ag-$n" >/dev/null 2>&1
  sleep 1
  $BIN send-keys -t "ag-$n:" "$TMPDIR/report-$n.sh" Enter
done
$BIN new-session -d -s home >/dev/null 2>&1
sleep 2
(sleep 2; printf '\002A'; sleep 1; printf '/'; sleep 0.5; printf 'green'; sleep 0.8; printf '\r'; sleep 0.7; printf '\r'; sleep 1.5; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c8.out" sh -c "stty rows 30 cols 100; $BIN attach -t home" >/dev/null 2>&1 &
CL=$!
assert "a query in the agent panel picks the row whose message matches" 'until_session ag-green'
wait $CL

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
