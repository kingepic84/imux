#!/bin/bash
# The documented notification recipe (examples/notify-agent.sh, set as an agent-state-changed hook)
# notifies for an agent that is blocked or done in a pane nobody looks at, and for no other.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qnr.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
RECIPE="$(cd "$(dirname "$0")/.." && pwd)/examples/notify-agent.sh"
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

until_true() {
  local n=0
  while [ "$n" -lt "$2" ]; do
    eval "$1" && return 0
    sleep 0.25
    n=$((n + 1))
  done
  return 1
}

printf '%s\n' '#!/bin/sh' 'printf "%s|%s\n" "$1" "$2" >>"$IMUX_TEST_NOTES"' >"$TMPDIR/fake-notify"
chmod +x "$TMPDIR/fake-notify"
export IMUX_NOTIFY_CMD="$TMPDIR/fake-notify"
export IMUX_TEST_NOTES="$TMPDIR/notes"
printf "set-hook -g agent-state-changed \"run-shell -b '%s'\"\n" "$RECIPE" >"$IMUX_CONFIG"

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

report() {
  echo "sleep 1; $BIN pane report-agent --source $1 --agent fake --state $2 --message m; sleep 8"
}

echo "-- a session nobody attached"
$BIN kill-server >/dev/null 2>&1
sleep 0.5
$BIN new-session -d -s unseen -n w "$(report t blocked)" >/dev/null 2>&1
assert "a blocked agent in an unattached session notifies" 'until_true "grep -q \"^imux: blocked|unseen:w\$\" \"$TMPDIR/notes\" 2>/dev/null" 40'
$BIN new-window -d -t unseen: -n busy "$(report u working)" >/dev/null 2>&1
$BIN new-window -d -t unseen: -n fin "$(report v done)" >/dev/null 2>&1
assert "a done agent notifies" 'until_true "grep -q \"^imux: done|unseen:fin\$\" \"$TMPDIR/notes\" 2>/dev/null" 40'
assert "a working agent does not" '! grep -q "busy" "$TMPDIR/notes"'
$BIN kill-server >/dev/null 2>&1

echo "-- a session being looked at"
rm -f "$TMPDIR/notes"
sleep 0.5
(sleep 1; sleep 14) | timeout 30 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN new -s seen" >/dev/null 2>&1 &
until_true '$BIN ls 2>/dev/null | grep -q "^seen:"' 40
$BIN send-keys -t seen:0 "$BIN pane report-agent --source t --agent fake --state blocked --message m" Enter >/dev/null 2>&1
$BIN new-window -d -t seen: -n elsewhere "$(report u blocked)" >/dev/null 2>&1
assert "the window nobody looks at notifies" 'until_true "grep -q \"^imux: blocked|seen:elsewhere\$\" \"$TMPDIR/notes\" 2>/dev/null" 40'
sleep 1
assert "the pane being looked at does not" '[ "$(wc -l <"$TMPDIR/notes" | tr -d " ")" = 1 ]'

$BIN kill-server >/dev/null 2>&1
wait 2>/dev/null
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
