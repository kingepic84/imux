#!/bin/bash
# #[align=left|center|right] in status-format[N]: the three parts are drawn, in that order on the
# row, and the directive itself is never shown.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qa.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s sty -n one >/dev/null 2>&1
$BIN set -g status-format[0] '#[align=left]LEFTPART#[align=centre]MIDPART#[align=right]RIGHTPART' >/dev/null 2>&1

rm -f "$TMPDIR"/rec.*
(sleep 2; sleep 1; printf '\002d'; sleep 0.5) | IMUX_RECORD_OUTPUT="$TMPDIR/rec" timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t sty" >/dev/null 2>&1
cat "$TMPDIR"/rec.* >"$TMPDIR/all" 2>/dev/null

assert "the left part is drawn" 'LC_ALL=C grep -aq "LEFTPART" "$TMPDIR/all"'
assert "the center part is drawn" 'LC_ALL=C grep -aq "MIDPART" "$TMPDIR/all"'
assert "the right part is drawn" 'LC_ALL=C grep -aq "RIGHTPART" "$TMPDIR/all"'
assert "the directive is not shown" '! LC_ALL=C grep -aq "align=" "$TMPDIR/all"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
