#!/bin/bash
# status-left-style, status-right-style, window-status-style, -current-style, -last-style, and the
# old status-bg / status-fg spelling: each color is unique, so the bytes a client writes show
# which option reached the bar.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qy.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' \
  'set -g status-left LEFT' \
  'set -g status-right RIGHT' \
  'set -g status-left-style fg=colour201' \
  'set -g status-right-style fg=colour202' \
  'set -g window-status-style fg=colour203' \
  'set -g window-status-current-style fg=colour204' \
  'set -g window-status-last-style fg=colour205' \
  'set -g status-bg colour206' \
  'set -g status-justify centre' \
  >"$IMUX_CONFIG"

$BIN new-session -d -s sty -n one >/dev/null 2>&1
$BIN new-window -d -t sty -n two >/dev/null 2>&1
$BIN new-window -d -t sty -n three >/dev/null 2>&1
$BIN select-window -t sty:two >/dev/null 2>&1
$BIN select-window -t sty:three >/dev/null 2>&1

$BIN show-options -g status-justify >"$TMPDIR/o" 2>&1
assert "status-justify is a real option" 'grep -aqx "status-justify centre" "$TMPDIR/o"'
$BIN show-options -g status-style >"$TMPDIR/o" 2>&1
assert "status-bg changed status-style" 'grep -aq "bg=colour206" "$TMPDIR/o"'

: >"$TMPDIR/rec"
(sleep 2; sleep 1; printf '\002d'; sleep 0.5) | IMUX_RECORD_OUTPUT="$TMPDIR/rec" timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t sty" >/dev/null 2>&1
cat "$TMPDIR"/rec.* >"$TMPDIR/all" 2>/dev/null
assert "status-left-style reaches the left segment" 'LC_ALL=C grep -aq "38;5;201" "$TMPDIR/all"'
assert "status-right-style reaches the right segment" 'LC_ALL=C grep -aq "38;5;202" "$TMPDIR/all"'
assert "window-status-style reaches the other tabs" 'LC_ALL=C grep -aq "38;5;203" "$TMPDIR/all"'
assert "window-status-current-style reaches the current tab" 'LC_ALL=C grep -aq "38;5;204" "$TMPDIR/all"'
assert "window-status-last-style reaches the last tab" 'LC_ALL=C grep -aq "38;5;205" "$TMPDIR/all"'
assert "status-bg colors the bar" 'LC_ALL=C grep -aq "48;5;206" "$TMPDIR/all"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
