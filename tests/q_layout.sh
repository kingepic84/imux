#!/bin/bash
# Layout strings (`#{window_layout}`, `select-layout "string"`), `select-layout -t`, `resize-pane -t -x -y`,
# `move-window` and `swap-window` with `session:N` targets, from the socket with no client attached.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-ql.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s lay -n a >/dev/null 2>&1
$BIN split-window -d -t lay:0 >/dev/null 2>&1
$BIN split-window -d -t lay:0 >/dev/null 2>&1

layout() { $BIN display-message -p -t "$1" '#{window_layout}' 2>/dev/null; }
widths() { $BIN list-panes -t "$1" -F '#{pane_width}' 2>/dev/null | tr '\n' ' '; }
heights() { $BIN list-panes -t "$1" -F '#{pane_height}' 2>/dev/null | tr '\n' ' '; }

assert "window_layout is a classic layout string" 'layout lay:0 | grep -Eq "^[0-9a-f]{4},[0-9]+x[0-9]+,0,0[\[{]"'

$BIN select-layout -t lay:0 even-horizontal >/dev/null 2>&1
assert "select-layout -t takes a window and a preset" 'layout lay:0 | grep -q "{"'
saved=$(layout lay:0)
saved_widths=$(widths lay:0)

$BIN select-layout -t lay:0 even-vertical >/dev/null 2>&1
assert "another preset changes the layout" '[ "$(layout lay:0)" != "$saved" ]'

$BIN select-layout -t lay:0 "$saved" >/dev/null 2>&1
assert "the saved layout string puts the layout back" '[ "$(layout lay:0)" = "$saved" ] && [ "$(widths lay:0)" = "$saved_widths" ]'

$BIN select-layout -t lay:0 "0000,80x24,0,0{40x24,0,0,0,39x24,41,0,1}" >/dev/null 2>&1
assert "a layout string with a wrong checksum or pane count is refused and nothing changes" '[ "$(layout lay:0)" = "$saved" ]'

$BIN select-layout -t lay:0 nonsense >/dev/null 2>&1
assert "an unknown layout name is an error" '! $BIN select-layout -t lay:0 nonsense >/dev/null 2>&1'

$BIN select-layout -t lay:0 even-horizontal >/dev/null 2>&1
first=$($BIN list-panes -t lay:0 -F '#{pane_id}' 2>/dev/null | head -1)
before=$(widths lay:0)
$BIN resize-pane -t "$first" -x 50 >/dev/null 2>&1
after=$(widths lay:0)
assert "resize-pane -t -x makes that pane 50 wide" '[ "$(echo "$after" | cut -d" " -f1)" = 50 ] && [ "$before" != "$after" ]'

$BIN select-layout -t lay:0 even-vertical >/dev/null 2>&1
$BIN resize-pane -t "$first" -y 12 >/dev/null 2>&1
assert "resize-pane -t -y makes that pane 12 high" '[ "$(heights lay:0 | cut -d" " -f1)" = 12 ]'

$BIN resize-pane -t "$first" -D 2 >/dev/null 2>&1
assert "resize-pane -t -D 2 grows it by two" '[ "$(heights lay:0 | cut -d" " -f1)" = 14 ]'

assert "resize-pane -x with no number is refused" '! $BIN resize-pane -t "$first" -x >/dev/null 2>&1'
assert "resize-pane -x 0 is refused" '! $BIN resize-pane -t "$first" -x 0 >/dev/null 2>&1'

$BIN new-window -t lay -n b >/dev/null 2>&1
$BIN new-window -t lay -n c >/dev/null 2>&1
order() { $BIN list-windows -t lay -F '#{window_name}' 2>/dev/null | tr '\n' ' '; }
assert "three windows a b c" '[ "$(order)" = "a b c " ]'

$BIN move-window -s lay:0 -t lay:2 >/dev/null 2>&1
assert "move-window -s lay:0 -t lay:2 moves a behind c" '[ "$(order)" = "b c a " ]'

$BIN swap-window -s lay:0 -t lay:2 >/dev/null 2>&1
assert "swap-window -s lay:0 -t lay:2 trades b and a" '[ "$(order)" = "a c b " ]'

$BIN new-session -d -s other -n x >/dev/null 2>&1
$BIN move-window -s lay:1 -t other:5 >/dev/null 2>&1
assert "move-window across sessions takes the window out of the first" '[ "$(order)" = "a b " ]'
assert "and puts it into the second" '[ "$($BIN list-windows -t other -F "#{window_name}" 2>/dev/null | tr "\n" " ")" = "x c " ]'

assert "swap-window across sessions is refused" '! $BIN swap-window -s lay:0 -t other:0 >/dev/null 2>&1'
$BIN new-session -d -s solo -n only >/dev/null 2>&1
assert "moving the only window of a session is refused" '! $BIN move-window -s solo:0 -t lay:5 >/dev/null 2>&1 && [ "$($BIN list-windows -t solo -F "#{window_name}" 2>/dev/null)" = only ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
