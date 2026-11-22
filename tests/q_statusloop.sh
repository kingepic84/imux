#!/bin/bash
# #{W:fmt,current_fmt} in a status-format row: one turn per window, the second format for the
# current window, loop_last_flag for the separator, window formats read through #{E:...}.
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
ESC=$(printf '\033')

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s s -n one >/dev/null 2>&1
$BIN new-window -d -t s -n two >/dev/null 2>&1
$BIN new-window -d -t s -n three >/dev/null 2>&1
$BIN select-window -t s:two >/dev/null 2>&1
$BIN set -g status-format[0] '<#{W:#I.#W#{?loop_last_flag,,+},[#I.#W]}>' >/dev/null 2>&1
$BIN set -g status-format[1] '#{W:#{E:window-status-format}#{?loop_last_flag,,#{window-status-separator}},#{E:window-status-current-format}#{?loop_last_flag,,#{window-status-separator}}}' >/dev/null 2>&1
$BIN set -g window-status-format 'a#W' >/dev/null 2>&1
$BIN set -g window-status-current-format 'C#W' >/dev/null 2>&1
$BIN set -g window-status-separator '|' >/dev/null 2>&1

(sleep 4; printf '\002d'; sleep 0.5) \
  | timeout 40 script -q "$TMPDIR/c1.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1
assert "each window takes a turn, the current one with its own format" 'grep -aqE "<0\.one\+\[1\.two\]2\.three>" "$TMPDIR/c1.out"'
assert "window formats and the separator are read through E:" 'grep -aqE "aone\|Ctwo\|athree" "$TMPDIR/c1.out"'

$BIN set -g status-format[0] '#{W:x' >/dev/null 2>&1
assert "an unterminated loop is only text" '[ $? -eq 0 ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
