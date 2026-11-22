#!/bin/bash
# select-pane -d (what a client types does not reach the pane) and -e (it does again); send-keys is
# not held back; #{pane_input_off}.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qio.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

OUT="$TMPDIR/out"
$BIN new-session -d -s io "cat >$OUT" >/dev/null 2>&1
sleep 1
flag() { $BIN display-message -p -t io: '#{pane_input_off}' 2>/dev/null; }

assert "input is on to start with" '[ "$(flag)" = 0 ]'

client() {
  (sleep 2; printf '%s\r' "$1"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t io" >/dev/null 2>&1
}
client typed_one
assert "a client types into the pane" 'grep -q typed_one "$OUT"'

$BIN select-pane -d -t io: >"$TMPDIR/o1" 2>&1
assert "select-pane -d turns it off" '[ "$(flag)" = 1 ] && [ ! -s "$TMPDIR/o1" ]'
client typed_two
assert "what a client types is dropped" '! grep -q typed_two "$OUT"'

$BIN send-keys -t io: -l sent_three >/dev/null 2>&1
$BIN send-keys -t io: Enter >/dev/null 2>&1
sleep 1
assert "send-keys still reaches the pane" 'grep -q sent_three "$OUT"'

$BIN select-pane -e -t io: >"$TMPDIR/o2" 2>&1
assert "select-pane -e turns it on" '[ "$(flag)" = 0 ]'
client typed_four
assert "typing works again" 'grep -q typed_four "$OUT"'

$BIN select-pane -d -e -t io: >"$TMPDIR/o3" 2>&1
assert "-d with -e is an error" 'grep -q "not both" "$TMPDIR/o3"'

$BIN select-pane -T 'my title' -t io: >"$TMPDIR/o4" 2>&1
assert "select-pane -T sets the pane title" '[ "$($BIN display-message -p -t io: "#{pane_title}")" = "my title" ]'
$BIN select-pane -T "$(printf 'x\033[31my')" -t io: >"$TMPDIR/o5" 2>&1
assert "a title is cleaned of control characters" '[ "$($BIN display-message -p -t io: "#{pane_title}")" = "x[31my" ] || [ "$($BIN display-message -p -t io: "#{pane_title}")" = "xy" ] || ! $BIN display-message -p -t io: "#{pane_title}" | LC_ALL=C grep -aq "$(printf "\033")"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
