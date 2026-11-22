#!/bin/bash
# split-window -f (the new pane spans the whole window), join-pane -b -f, and swap-pane -s -t
# inside a window and between windows.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qfs.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s fs -n w >/dev/null 2>&1
width=$($BIN display-message -p -t fs:w '#{window_width}')
height=$($BIN display-message -p -t fs:w '#{window_height}')
v() { $BIN display-message -p -t "$1" "#{$2}" 2>/dev/null; }

first=$($BIN display-message -p -t fs:w '#{pane_id}')
side=$($BIN split-window -h -d -t "$first" -P -F '#{pane_id}')
assert "two panes side by side" '[ "$(v $first pane_width)" -lt "$width" ] && [ "$(v $side pane_top)" = 0 ]'

full=$($BIN split-window -v -f -d -t "$first" -P -F '#{pane_id}')
assert "split-window -f -v spans the whole width" '[ "$(v $full pane_width)" = "$width" ]'
assert "and sits below both" '[ "$(v $full pane_top)" -gt 0 ] && [ "$(v $first pane_top)" = 0 ]'

lefty=$($BIN split-window -h -f -b -d -l 10 -t "$first" -P -F '#{pane_id}')
assert "split-window -f -b -h puts the pane on the far left" '[ "$(v $lefty pane_left)" = 0 ]'
assert "with the full height" '[ "$(v $lefty pane_height)" = "$height" ]'
assert "and the size asked for" '[ "$(v $lefty pane_width)" = 10 ]'

top=$($BIN split-window -v -f -b -d -l 5 -t "$first" -P -F '#{pane_id}')
assert "split-window -f -b -v puts the pane on top, 5 rows" '[ "$(v $top pane_top)" = 0 ] && [ "$(v $top pane_height)" = 5 ]'
assert "spanning the width" '[ "$(v $top pane_width)" = "$width" ]'

$BIN new-session -d -s js -n a >/dev/null 2>&1
$BIN new-window -d -t js -n b >/dev/null 2>&1
base=$($BIN display-message -p -t js:a '#{pane_id}')
mover=$($BIN display-message -p -t js:b '#{pane_id}')
$BIN join-pane -h -b -s "$mover" -t "$base" >"$TMPDIR/jb" 2>&1
assert "join-pane -b put the moved pane before the target" '[ "$(v $mover pane_left)" = 0 ] && [ "$(v $base pane_left)" -gt 0 ]'
assert "the emptied window is gone" '[ "$($BIN list-windows -t js -F x | grep -c x)" = 1 ]'

$BIN new-window -d -t js -n c >/dev/null 2>&1
other=$($BIN display-message -p -t js:c '#{pane_id}')
$BIN join-pane -v -f -s "$other" -t "$base" >"$TMPDIR/jf" 2>&1
assert "join-pane -f spans the window" '[ "$(v $other pane_width)" = "$width" ] && [ "$(v $other pane_top)" -gt 0 ]'

$BIN new-session -d -s sw -n x >/dev/null 2>&1
a=$($BIN display-message -p -t sw:x '#{pane_id}')
b=$($BIN split-window -h -d -t "$a" -P -F '#{pane_id}')
order() { $BIN list-panes -t "$1" -F '#{pane_id}' | tr '\n' ' '; }
assert "before the swap a is first" '[ "$(order sw:x)" = "$a $b " ]'
$BIN swap-pane -s "$a" -t "$b" >"$TMPDIR/sp" 2>&1
assert "swap-pane -s -t trades places in a window" '[ "$(order sw:x)" = "$b $a " ]'
assert "and the left pane is now b" '[ "$(v $b pane_left)" = 0 ]'

$BIN new-window -d -t sw -n y >/dev/null 2>&1
c=$($BIN display-message -p -t sw:y '#{pane_id}')
$BIN swap-pane -s "$a" -t "$c" >"$TMPDIR/sp2" 2>&1
assert "swap-pane between windows moves each into the other" '[ "$(order sw:x)" = "$b $c " ] && [ "$(order sw:y)" = "$a " ]'
assert "the moved panes have the room of their new place" '[ "$(v $a pane_width)" = "$width" ] && [ "$(v $c pane_left)" -gt 0 ]'

$BIN swap-pane -s "$b" -t "$b" >"$TMPDIR/sp3" 2>&1
assert "a pane cannot swap with itself" '! grep -q . "$TMPDIR/sp3" || grep -q "same pane" "$TMPDIR/sp3"'
$BIN swap-pane -s "%99999" -t "$b" >"$TMPDIR/sp4" 2>&1
assert "an unknown pane is an error" 'grep -q "can.t find" "$TMPDIR/sp4"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
