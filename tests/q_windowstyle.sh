#!/bin/bash
# window-style (every pane) and window-active-style (the active pane on top of it): each color is
# unique, so the bytes a client writes show which pane took which.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qw.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

record() {
  : >"$TMPDIR/rec"
  rm -f "$TMPDIR"/rec.*
  (sleep 2; sleep 1; printf '\002d'; sleep 0.5) | IMUX_RECORD_OUTPUT="$TMPDIR/rec" timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t sty" >/dev/null 2>&1
  cat "$TMPDIR"/rec.* >"$TMPDIR/all" 2>/dev/null
}

printf '%s\n' \
  'set -g window-style bg=colour201' \
  'set -g window-active-style bg=colour202' \
  >"$IMUX_CONFIG"

$BIN new-session -d -s sty -n one >/dev/null 2>&1
$BIN split-window -d -t sty:one >/dev/null 2>&1
$BIN show-options -g window-style >"$TMPDIR/o" 2>&1
assert "window-style is a real option" 'grep -aq "bg=colour201" "$TMPDIR/o"'
$BIN show-options -g window-active-style >"$TMPDIR/o" 2>&1
assert "window-active-style is a real option" 'grep -aq "bg=colour202" "$TMPDIR/o"'

record
assert "the pane that is not active takes window-style" 'LC_ALL=C grep -aq "48;5;201" "$TMPDIR/all"'
assert "the active pane takes window-active-style" 'LC_ALL=C grep -aq "48;5;202" "$TMPDIR/all"'

$BIN set -g window-style 'default' >/dev/null 2>&1
$BIN set -g window-active-style 'default' >/dev/null 2>&1
record
assert "reset to default, no pane has the colors" '! LC_ALL=C grep -aq "48;5;20[12]" "$TMPDIR/all"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
