#!/bin/bash
# Mouse keys in `bind -n`: a press, the wheel, a drag, a second press, a modifier, the border, the
# status line, the variables a binding sees (#{mouse_x}, #{mouse_y}, #{mouse_word}), a menu opened
# at the pointer, an event with no binding going on to what imux does by itself, and the names in
# `list-keys` and the errors.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qmo.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
LOG="$TMPDIR/log"
: >"$LOG"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cat >"$IMUX_CONFIG" <<EOF
set -g status-right ""
bind -n MouseDown3Pane run-shell "echo DOWN3:#{mouse_x}:#{mouse_y}:#{mouse_any_flag} >> $LOG"
bind -n WheelUpPane run-shell "echo WHEELUP >> $LOG"
bind -n WheelDownStatusDefault run-shell "echo WHEELSTATUS >> $LOG"
bind -n MouseDown3StatusLeft run-shell "echo STATUSLEFT >> $LOG"
bind -n MouseDown3StatusDefault run-shell "echo STATUSDEFAULT >> $LOG"
bind -n MouseDrag1Pane run-shell "echo DRAG >> $LOG"
bind -n MouseDragEnd1Pane run-shell "echo DRAGEND >> $LOG"
bind -n DoubleClick1Pane run-shell "echo DOUBLE:#{q:mouse_word} >> $LOG"
bind -n C-MouseDown1Pane run-shell "echo CTRLDOWN >> $LOG"
bind -n MouseDown3Border run-shell "echo BORDER >> $LOG"
bind -n MouseDown2Pane display-menu -x M -y M -T Mm "Alpha" a "run-shell 'echo MENUA >> $LOG'"
EOF

$BIN new-session -d -s mo -n win >/dev/null 2>&1
$BIN split-window -h >/dev/null 2>&1
$BIN select-pane -t %0 >/dev/null 2>&1
sleep 1
W0=$($BIN list-panes -F '#{pane_width}' | head -1)
RIGHT_PANE_ACTIVE='[ "$($BIN list-panes -F "#{pane_id}:#{pane_active}" | grep -c "^%1:1")" = 1 ]'

$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "list-keys shows the mouse keys by name" 'grep -aq "MouseDown3Pane" "$TMPDIR/keys" && grep -aq "C-MouseDown1Pane" "$TMPDIR/keys" && grep -aq "WheelDownStatusDefault" "$TMPDIR/keys"'

say() {
  $BIN send-keys -t %0 -l "$1" >/dev/null 2>&1
  $BIN send-keys -t %0 Enter >/dev/null 2>&1
}
say 'echo MARKERWORD'
sleep 1
ROW=$($BIN capture-pane -p -t %0 | grep -n '^MARKERWORD$' | head -1 | cut -d: -f1)
ROW=${ROW:-0}

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 40 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t mo" >/dev/null 2>&1
}

k_basic() {
  W0=$($BIN list-panes -F '#{pane_width}' | head -1)
  printf '\033[<2;5;5M\033[<2;5;5m'
  sleep 0.5
  printf '\033[<64;5;5M'
  sleep 0.5
  printf '\033[<65;90;30M'
  sleep 0.5
  printf '\033[<2;2;30M\033[<2;2;30m'
  sleep 0.5
  printf '\033[<2;90;30M\033[<2;90;30m'
  sleep 0.5
  printf '\033[<16;5;5M\033[<16;5;5m'
  sleep 0.5
  printf '\033[<2;%d;5M\033[<2;%d;5m' "$((W0 + 1))" "$((W0 + 1))"
  sleep 0.5
}
client k_basic
assert "a press with the right button, with the pointer's place in the pane and the program's mouse flag" 'grep -aq "^DOWN3:4:4:0$" "$LOG"'
assert "the wheel over a pane" 'grep -aq "^WHEELUP$" "$LOG"'
assert "the wheel over the empty part of the status line" 'grep -aq "^WHEELSTATUS$" "$LOG"'
assert "a press on the session part of the status line" 'grep -aq "^STATUSLEFT$" "$LOG"'
assert "a press on the empty part of the status line" 'grep -aq "^STATUSDEFAULT$" "$LOG"'
assert "a modifier is part of the key" 'grep -aq "^CTRLDOWN$" "$LOG"'
assert "a press on the line between two panes" 'grep -aq "^BORDER$" "$LOG"'

k_drag() {
  printf '\033[<0;5;5M'
  sleep 0.3
  printf '\033[<32;8;5M'
  sleep 0.3
  printf '\033[<0;8;5m'
  sleep 0.5
}
client k_drag
assert "a drag and the end of it, by the place the drag began" 'grep -aq "^DRAG$" "$LOG" && grep -aq "^DRAGEND$" "$LOG"'

k_double() {
  ROW=$($BIN capture-pane -p -t %0 | grep -n '^MARKERWORD$' | head -1 | cut -d: -f1)
  ROW=${ROW:-1}
  printf '\033[<0;3;%dM\033[<0;3;%dm\033[<0;3;%dM\033[<0;3;%dm' "$ROW" "$ROW" "$ROW" "$ROW"
  sleep 0.7
}
client k_double
assert "a second press on the same cell, with the word under the pointer" 'grep -aq "^DOUBLE:MARKERWORD$" "$LOG"'

$BIN select-pane -t %0 >/dev/null 2>&1
k_fall() {
  W0=$($BIN list-panes -F '#{pane_width}' | head -1)
  printf '\033[<0;%d;5M\033[<0;%d;5m' "$((W0 + 5))" "$((W0 + 5))"
  sleep 0.7
}
client k_fall
assert "a press that has no binding goes on to the focus change" "$RIGHT_PANE_ACTIVE"

k_menu() {
  printf '\033[<1;30;10M\033[<1;30;10m'
  sleep 0.8
  printf 'a'
  sleep 0.7
}
client k_menu
assert "a menu opened by a mouse key takes its keys, placed at the pointer" 'grep -aq "^MENUA$" "$LOG" && LC_ALL=C grep -aq "Mm" "$TMPDIR/c.out"'

printf 'bind -n MouseDown4Pane display-message x\nbind -n MouseDown1 display-message x\n' >"$TMPDIR/bad.conf"
$BIN source-file "$TMPDIR/bad.conf" >"$TMPDIR/e1" 2>&1
assert "a mouse key name needs a button from 1 to 3 and a place" 'grep -aq "unknown key: MouseDown4Pane" "$TMPDIR/e1"'
$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
