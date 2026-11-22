#!/bin/bash
# pane-border-lines: the characters a client is sent for the divider between two panes.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qb.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

record() {
  rm -f "$TMPDIR"/rec.*
  (sleep 2; sleep 1; printf '\002d'; sleep 0.5) | IMUX_RECORD_OUTPUT="$TMPDIR/rec" timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t sty" >/dev/null 2>&1
  cat "$TMPDIR"/rec.* >"$TMPDIR/all" 2>/dev/null
}

$BIN new-session -d -s sty -n one >/dev/null 2>&1
$BIN split-window -d -h -t sty:one >/dev/null 2>&1

record
assert "the default divider is the single line" 'grep -aq "│" "$TMPDIR/all" && ! grep -aq "║" "$TMPDIR/all"'

$BIN set -g pane-border-lines double >/dev/null 2>&1
record
assert "double draws the double line" 'grep -aq "║" "$TMPDIR/all" && ! grep -aq "│" "$TMPDIR/all"'

$BIN set -g pane-border-lines heavy >/dev/null 2>&1
record
assert "heavy draws the heavy line" 'grep -aq "┃" "$TMPDIR/all"'

$BIN set -g pane-border-lines simple >/dev/null 2>&1
record
assert "simple draws plain characters" '! grep -aq "│" "$TMPDIR/all" && ! grep -aq "┃" "$TMPDIR/all"'

$BIN set -g pane-border-lines wavy >"$TMPDIR/o" 2>&1
assert "a name that is not a line set is refused" 'grep -aqi "pane-border-lines" "$TMPDIR/o"'
$BIN show-options -g pane-border-lines >"$TMPDIR/o" 2>&1
assert "the refused value changed nothing" 'grep -aqx "pane-border-lines simple" "$TMPDIR/o"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
