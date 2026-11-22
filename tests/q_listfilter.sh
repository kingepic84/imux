#!/bin/bash
# -f FILTER on list-sessions, list-windows, list-panes and list-clients: a line stays when the
# filter format expands to something other than nothing or 0.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qlf.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s alpha -n one >/dev/null 2>&1
$BIN new-session -d -s beta -n two >/dev/null 2>&1
$BIN new-window -t alpha -n three >/dev/null 2>&1
$BIN split-window -d -t alpha:one >/dev/null 2>&1

flat() { tr '\n' ' '; }

out=$($BIN list-sessions -F '#{session_name}' | flat)
assert "no filter lists every session" '[ "$out" = "alpha beta " ]'

out=$($BIN list-sessions -f '#{==:#{session_name},beta}' -F '#{session_name}' | flat)
assert "list-sessions -f keeps the match" '[ "$out" = "beta " ]'

out=$($BIN list-sessions -f '0' -F '#{session_name}' | flat)
assert "a filter of 0 keeps nothing" '[ -z "$out" ]'

out=$($BIN list-sessions -f '' -F '#{session_name}' | flat)
assert "an empty filter keeps nothing" '[ -z "$out" ]'

out=$($BIN list-windows -t alpha -f '#{m:t*,#{window_name}}' -F '#{window_name}' | flat)
assert "list-windows -f with a glob" '[ "$out" = "three " ]'

out=$($BIN list-windows -a -f '#{==:#{session_name},beta}' -F '#{window_name}' | flat)
assert "list-windows -a -f picks across sessions" '[ "$out" = "two " ]'

out=$($BIN list-panes -t alpha:one -f '#{pane_active}' -F '#{pane_index}' | flat)
assert "list-panes -f keeps the active pane" '[ "$out" = "1 " ] || [ "$out" = "0 " ]'

out=$($BIN list-panes -t alpha:one -f '#{?pane_active,0,1}' -F '#{pane_index}' | flat)
other=$($BIN list-panes -t alpha:one -f '#{pane_active}' -F '#{pane_index}' | flat)
assert "the two filters split the panes" '[ -n "$out" ] && [ -n "$other" ] && [ "$out" != "$other" ]'

out=$($BIN list-panes -a -f '#{==:#{window_name},two}' -F '#{pane_id}' | flat)
assert "list-panes -a -f" '[ "$(echo $out | wc -w | tr -d " ")" = 1 ]'

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t beta" >/dev/null 2>&1
}
probe() {
  $BIN list-clients -F '#{client_session}' >"$TMPDIR/lc.all" 2>&1
  $BIN list-clients -f '#{==:#{client_session},beta}' -F '#{client_session}' >"$TMPDIR/lc.yes" 2>&1
  $BIN list-clients -f '#{==:#{client_session},alpha}' -F '#{client_session}' >"$TMPDIR/lc.no" 2>&1
}
client probe
assert "list-clients sees the client" 'grep -q beta "$TMPDIR/lc.all"'
assert "list-clients -f keeps the matching client" 'grep -q beta "$TMPDIR/lc.yes"'
assert "list-clients -f drops the other" '[ ! -s "$TMPDIR/lc.no" ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
