#!/bin/bash
# copy-mode -t pane (any pane, the active one unchanged), -u (a page back), -H (no position at the top
# right) and -e, from the socket.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qcm.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 140; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

fresh() {
  $BIN kill-session -t cp >/dev/null 2>&1
  $BIN new-session -d -s cp -n main "seq 1 200; sleep 100" >/dev/null 2>&1
  $BIN new-window -d -t cp -n other >/dev/null 2>&1
  sleep 1
}
mode() { $BIN display-message -p -t "$1" '#{pane_in_mode}' 2>/dev/null; }

fresh
$BIN copy-mode -t cp:other >"$TMPDIR/o1" 2>&1
assert "copy-mode -t puts a pane of another window in copy mode" '[ "$(mode cp:other)" = 1 ]'
assert "and leaves the current window alone" '[ "$($BIN display-message -p -t cp: "#{window_name}")" = main ] && [ "$(mode cp:main)" = 0 ]'

$BIN copy-mode -t cp:other >"$TMPDIR/o2" 2>&1
assert "again is no error" '[ ! -s "$TMPDIR/o2" ]'

$BIN copy-mode -t cp:nowhere >"$TMPDIR/o3" 2>&1
assert "an unknown target is an error" 'grep -q "can.t find" "$TMPDIR/o3"'
$BIN copy-mode -x >"$TMPDIR/o4" 2>&1
assert "an unknown flag is an error" 'grep -q "unknown flag\|usage" "$TMPDIR/o4"'

watch() {
  rm -f "$TMPDIR"/rec.*
  (sleep 2; "$@"; sleep 1.5; printf '\002d'; sleep 0.5) | IMUX_RECORD_OUTPUT="$TMPDIR/rec" timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t cp" >/dev/null 2>&1
  command cat "$TMPDIR"/rec.* >"$TMPDIR/all" 2>/dev/null
}
select_main() { $BIN select-window -t cp:main >/dev/null 2>&1; }

fresh
select_main_then() { select_main; "$@"; }
watch select_main_then $BIN copy-mode
assert "plain: the position is [0/n]" 'LC_ALL=C grep -aEq "\[0/[0-9]+\]" "$TMPDIR/all"'

fresh
watch select_main_then $BIN copy-mode -u
assert "-u starts a page back" 'LC_ALL=C grep -aEq "\[[1-9][0-9]*/[0-9]+\]" "$TMPDIR/all"'
assert "-u is not at the bottom" '! LC_ALL=C grep -aEq "\[0/[0-9]+\]" "$TMPDIR/all"'

fresh
watch select_main_then $BIN copy-mode -H
assert "-H leaves the position out" '! LC_ALL=C grep -aEq "\[[0-9]+/[0-9]+\]" "$TMPDIR/all"'
assert "-H still is copy mode" '[ "$(mode cp:main)" = 1 ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
