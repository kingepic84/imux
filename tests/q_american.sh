#!/bin/bash
# imux spells color, center and their relatives the American way; the spellings tmux uses
# (clock-mode-colour, status-justify centre, #[align=centre], popup_centre_x, colourN) keep working,
# so an existing tmux configuration loads the same.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qa.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 60; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\n' >"$IMUX_CONFIG"
$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s am >/dev/null 2>&1

echo "-- option names"
assert "the American option name is accepted" '$BIN set -g clock-mode-color red >/dev/null 2>&1'
assert "and shows its value" '[ "$($BIN show-options -gv clock-mode-color 2>/dev/null)" = red ]'
assert "the old name is accepted" '$BIN set -g clock-mode-colour green >/dev/null 2>&1'
assert "and is the same option" '[ "$($BIN show-options -gv clock-mode-color 2>/dev/null)" = green ]'
assert "an ignored option answers to both spellings" '$BIN set -g pane-colours "1 2" >/dev/null 2>&1 && $BIN set -g pane-colors "1 2" >/dev/null 2>&1'

echo "-- values"
assert "status-justify center" '$BIN set -g status-justify center >/dev/null 2>&1'
assert "status-justify centre" '$BIN set -g status-justify centre >/dev/null 2>&1'
assert "status-justify absolute-center" '$BIN set -g status-justify absolute-center >/dev/null 2>&1'
assert "colorN and colourN are one color" '$BIN set -g status-style fg=color33 >/dev/null 2>&1 && $BIN set -g status-style fg=colour33 >/dev/null 2>&1'

echo "-- formats (the popup_center_x variables are covered by a unit test in server/position.rs)"
assert "a status row accepts align=center and align=centre" '$BIN set -g "status-format[0]" "#[align=left]L#[align=center]M#[align=centre]N#[align=right]R" >/dev/null 2>&1'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
