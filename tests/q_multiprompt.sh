#!/bin/bash
# command-prompt -p "a,b": prompts asked one after the other, answers as %1..%9 (and %% the last),
# -I with a text per prompt, -T accepted, and a confirm-before label with a comma stays one label.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qmp.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' \
  'bind a command-prompt -p "first:,second:" "rename-window %1-%2"' \
  'bind i command-prompt -I "one,two" -p "x:,y:" "rename-window %2%1"' \
  'bind l command-prompt -p "a:,b:" "rename-window %%"' \
  'bind t command-prompt -T command -p "cmd:" "rename-window t%%"' \
  'bind c confirm-before -p "sure, really?" "rename-window sure"' \
  'bind p command-prompt -p "n:" "rename-window %%%1%2"' \
  >"$IMUX_CONFIG"

$BIN new-session -d -s mp -n start >/dev/null 2>&1
current() { $BIN display-message -p '#{window_name}' 2>/dev/null; }
client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t mp" >/dev/null 2>&1
}

k_two() { printf '\002a'; sleep 0.5; printf 'AA\r'; sleep 0.5; printf 'BB\r'; }
client k_two
assert "two prompts, two answers: %1-%2" '[ "$(current)" = AA-BB ]'
assert "the second label was shown" 'LC_ALL=C grep -aq "second:" "$TMPDIR/c.out"'

k_initial() { printf '\002i'; sleep 0.5; printf '\r'; sleep 0.5; printf '\r'; }
client k_initial
assert "-I gives each prompt its own first text, %2%1" '[ "$(current)" = twoone ]'

k_last() { printf '\002l'; sleep 0.5; printf 'x\r'; sleep 0.5; printf 'y\r'; }
client k_last
assert "%% is the last answer" '[ "$(current)" = y ]'

k_cancel_second() { printf '\002a'; sleep 0.5; printf 'one\r'; sleep 0.5; printf 'half'; sleep 0.3; printf '\033'; }
client k_cancel_second
assert "Escape at the second prompt cancels everything" '[ "$(current)" = y ]'

k_type() { printf '\002t'; sleep 0.5; printf 'ok\r'; }
client k_type
assert "-T is accepted" '[ "$(current)" = tok ]'

k_confirm() { printf '\002c'; sleep 0.5; printf 'y'; }
client k_confirm
assert "a confirm label with a comma is one question" '[ "$(current)" = sure ]'

k_pct() { printf '\002p'; sleep 0.5; printf '%%2\r'; }
client k_pct
assert "what is typed is not read for % again" '[ "$(current)" = "%2%2" ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
