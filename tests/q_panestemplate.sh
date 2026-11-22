#!/bin/bash
# display-panes [-d ms] template: the number chosen runs the template with %% the pane's id.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qpt.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf '%s\n' \
  'bind Q display-panes "set -g @picked %%"' \
  'bind W display-panes -d 3000 "set -g @timed %%"' \
  'bind E display-panes' \
  >"$IMUX_CONFIG"

$BIN new-session -d -s pt -n w >/dev/null 2>&1
first=$($BIN display-message -p -t pt: '#{pane_id}')
second=$($BIN split-window -h -d -t pt: -P -F '#{pane_id}')
third=$($BIN split-window -h -d -t pt: -P -F '#{pane_id}')

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t pt" >/dev/null 2>&1
}
active() { $BIN display-message -p -t pt: '#{pane_id}'; }
picked() { $BIN show-options -gqv "$1" 2>/dev/null; }

$BIN select-pane -t "$first" >/dev/null 2>&1
k_run() { printf '\002Q'; sleep 0.5; printf '1'; }
client k_run
assert "the chosen number runs the template with the pane id" '[ "$(picked @picked)" = "$($BIN list-panes -t pt: -F "#{pane_index} #{pane_id}" | grep "^1 " | cut -d" " -f2)" ]'
assert "and the pane is not selected" '[ "$(active)" = "$first" ]'

k_timed() { printf '\002W'; sleep 0.5; printf '2'; }
client k_timed
assert "-d with a template works" '[ -n "$(picked @timed)" ]'

k_plain() { printf '\002E'; sleep 0.5; printf '2'; }
client k_plain
assert "without a template the pane is selected as before" '[ "$(active)" != "$first" ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
