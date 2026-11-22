#!/bin/bash
# display-panes-color and display-panes-active-color: the backgrounds of the pane numbers that
# prefix q draws. Each color is unique, so the bytes a client writes show which label took which.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qp.XXXXXX)
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
  (sleep 2; printf '\002q'; sleep 0.6; printf '\033'; sleep 0.4; printf '\002d'; sleep 0.5) | IMUX_RECORD_OUTPUT="$TMPDIR/rec" timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t sty" >/dev/null 2>&1
  cat "$TMPDIR"/rec.* >"$TMPDIR/all" 2>/dev/null
}

$BIN new-session -d -s sty -n one >/dev/null 2>&1
$BIN split-window -d -h -t sty:one >/dev/null 2>&1

record
assert "built-in colors: neither option's color shows" '! LC_ALL=C grep -aq "48;5;20[12]" "$TMPDIR/all"'

$BIN set -g display-panes-color colour201 >/dev/null 2>&1
$BIN set -g display-panes-active-color colour202 >/dev/null 2>&1
record
assert "the other pane's label takes display-panes-color" 'LC_ALL=C grep -aq "48;5;201" "$TMPDIR/all"'
assert "the active pane's label takes display-panes-active-color" 'LC_ALL=C grep -aq "48;5;202" "$TMPDIR/all"'

$BIN set -g display-panes-color "not a color" >"$TMPDIR/o" 2>&1
assert "a bad color is refused" 'grep -aqi "display-panes-color" "$TMPDIR/o"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
