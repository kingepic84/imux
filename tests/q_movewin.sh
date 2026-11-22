#!/bin/bash
# move-window -a (after the target), -k (kill the window in the way) and -r (renumber, nothing to do).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qmw.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

order() { $BIN list-windows -t "$1" -F '#{window_name}' 2>/dev/null | tr '\n' ' '; }
fresh() {
  $BIN kill-session -t "$1" >/dev/null 2>&1
  $BIN new-session -d -s "$1" -n a >/dev/null 2>&1
  for n in b c d; do $BIN new-window -d -t "$1" -n "$n" >/dev/null 2>&1; done
}
$BIN new-session -d -s keep -n k >/dev/null 2>&1

fresh m
$BIN move-window -a -s m:a -t m:c >"$TMPDIR/o1" 2>&1
assert "-a moves a window after a later target" '[ "$(order m)" = "b c a d " ]'

fresh m
$BIN move-window -a -s m:d -t m:a >"$TMPDIR/o2" 2>&1
assert "-a moves a window after an earlier target" '[ "$(order m)" = "a d b c " ]'

fresh m
$BIN move-window -s m:d -t m:b >"$TMPDIR/o3" 2>&1
assert "without -a the window takes the place of the target" '[ "$(order m)" = "a d b c " ]'

fresh m
$BIN move-window -k -s m:d -t m:b >"$TMPDIR/o4" 2>&1
assert "-k kills the window in the way" '[ "$(order m)" = "a d c " ]'

fresh m
$BIN move-window -r -t m: >"$TMPDIR/o5" 2>&1
assert "-r with a target is accepted and changes nothing" '[ "$(order m)" = "a b c d " ] && [ ! -s "$TMPDIR/o5" ]'
$BIN move-window -r >"$TMPDIR/o6" 2>&1
assert "-r alone is accepted" '[ "$?" = 0 ] && [ ! -s "$TMPDIR/o6" ]'

fresh x
$BIN new-session -d -s y -n r >/dev/null 2>&1
$BIN new-window -d -t y -n s >/dev/null 2>&1
$BIN move-window -a -s y:r -t x:b >"$TMPDIR/o7" 2>&1
assert "-a across sessions" '[ "$(order x)" = "a b r c d " ]'

fresh m
$BIN move-window -a -k -s m:a -t m:c >"$TMPDIR/o8" 2>&1
assert "-a with -k kills nothing" '[ "$(order m)" = "b c a d " ]'

fresh m
$BIN swap-window -k -s m:a -t m:b >"$TMPDIR/o9" 2>&1
assert "swap-window takes no -k" 'grep -q . "$TMPDIR/o9" && [ "$(order m)" = "a b c d " ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
