#!/bin/bash
# bind -n MouseDrag1Border resize-pane -M: the stock mouse line means "do what a border drag does",
# so the built-in drag still works; any other command bound to that key takes the drag over.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qrm.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

run_with() {
  printf '%s\n' "$1" >"$IMUX_CONFIG"
  $BIN kill-server >/dev/null 2>&1
  sleep 0.5
  $BIN new-session -d -s rm -n w >/dev/null 2>&1
  $BIN split-window -h -d -t rm: >/dev/null 2>&1
  W0=$($BIN list-panes -t rm: -F '#{pane_width}' | head -1)
  drag() {
    printf '\033[<0;%d;5M' "$((W0 + 1))"
    sleep 0.3
    printf '\033[<32;%d;5M' "$((W0 + 6))"
    sleep 0.3
    printf '\033[<32;%d;5M' "$((W0 + 11))"
    sleep 0.3
    printf '\033[<0;%d;5m' "$((W0 + 11))"
    sleep 0.5
  }
  (sleep 2; drag; sleep 1; printf '\002d'; sleep 0.5) | timeout 40 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t rm" >/dev/null 2>&1
  W1=$($BIN list-panes -t rm: -F '#{pane_width}' | head -1)
}

run_with 'bind -n MouseDrag1Border resize-pane -M'
assert "the binding loaded" '$BIN list-keys | grep -aq "MouseDrag1Border"'
assert "the left pane got wider by the drag" '[ "$W1" -gt "$W0" ]'
assert "and no error was shown for the command" '! LC_ALL=C grep -aq "unknown flag\|usage: resize" "$TMPDIR/c.out"'

run_with 'set -g @none 1'
assert "with no binding the drag resizes as before" '[ "$W1" -gt "$W0" ]'

$BIN resize-pane -M >"$TMPDIR/o" 2>&1
assert "resize-pane -M from the shell is no resize" 'grep -q "unknown flag\|usage" "$TMPDIR/o"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
