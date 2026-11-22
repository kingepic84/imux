#!/bin/bash
# source-file -q (a missing file is no error), -n (read and check only), -v (log each line), and a
# pattern in the file name.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qsf.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export IMUX_LOG_FILE="$TMPDIR/log"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

D="$TMPDIR/conf"
mkdir -p "$D"
printf '%s\n' 'set -g @one 1' >"$D/a1.conf"
printf '%s\n' 'set -g @two 2' >"$D/a2.conf"
printf '%s\n' 'set -g @other 9' >"$D/b.conf"
printf '%s\n' 'set -g @hidden 7' >"$D/.a3.conf"
printf '%s\n' 'set -g @three 3' 'frobnicate now' >"$D/bad.conf"
printf '%s\n' 'set -g @four 4' >"$D/ok.conf"

$BIN new-session -d -s sf >/dev/null 2>&1
opt() { $BIN show-options -gqv "$1" 2>/dev/null; }

$BIN source-file "$D/nothing.conf" >"$TMPDIR/o1" 2>&1
assert "a missing file is an error" '[ "$?" != 0 ] || grep -q "No such file" "$TMPDIR/o1"'

$BIN source-file -q "$D/nothing.conf" >"$TMPDIR/o2" 2>&1
assert "-q makes a missing file no error" '[ "$?" = 0 ] && [ ! -s "$TMPDIR/o2" ]'

$BIN source-file "$D/a*.conf" >"$TMPDIR/o3" 2>&1
assert "a pattern reads every match" '[ "$(opt @one)" = 1 ] && [ "$(opt @two)" = 2 ]'
assert "and no other file" '[ -z "$(opt @other)" ]'
assert "a hidden file is not matched by a plain pattern" '[ -z "$(opt @hidden)" ]'

$BIN source-file '.a*.conf' >/dev/null 2>&1
$BIN source-file "$D/.a*.conf" >"$TMPDIR/o3b" 2>&1
assert "a pattern that starts with a dot matches hidden files" '[ "$(opt @hidden)" = 7 ]'

$BIN source-file "$D/z*.conf" >"$TMPDIR/o4" 2>&1
assert "a pattern without a match is an error" 'grep -q "no file matches" "$TMPDIR/o4"'
$BIN source-file -q "$D/z*.conf" >"$TMPDIR/o5" 2>&1
assert "-q makes it quiet" '[ "$?" = 0 ] && [ ! -s "$TMPDIR/o5" ]'

$BIN source-file "$TMPDIR/co*/a1.conf" >"$TMPDIR/o6" 2>&1
assert "a pattern in a directory part matches nothing" 'grep -q "no file matches" "$TMPDIR/o6"'

$BIN source-file -n "$D/ok.conf" >"$TMPDIR/o7" 2>&1
assert "-n of a good file is quiet" '[ "$?" = 0 ] && [ ! -s "$TMPDIR/o7" ]'
assert "and changes nothing" '[ -z "$(opt @four)" ]'

$BIN source-file -n "$D/bad.conf" >"$TMPDIR/o8" 2>&1
assert "-n names the file and line of a bad one" 'grep -q "bad.conf:2" "$TMPDIR/o8"'
assert "and changes nothing there either" '[ -z "$(opt @three)" ]'

$BIN source-file -nq "$D/ok.conf" >"$TMPDIR/o9" 2>&1
assert "flags can be together" '[ "$?" = 0 ] && [ -z "$(opt @four)" ]'

$BIN source-file -v "$D/ok.conf" >"$TMPDIR/o10" 2>&1
assert "-v reads and applies" '[ "$(opt @four)" = 4 ]'
assert "and logs the line" 'grep -q "source-file: .*ok.conf:1: set -g @four 4" "$IMUX_LOG_FILE"'

$BIN source-file -z "$D/ok.conf" >"$TMPDIR/o11" 2>&1
assert "an unknown flag is a usage error" 'grep -q "usage: source-file" "$TMPDIR/o11"'

printf '%s\n' "source-file -q $D/absent.conf" "source-file $D/ghost.conf" >"$TMPDIR/main.conf"
$BIN check-config "$TMPDIR/main.conf" >"$TMPDIR/o12" 2>&1
assert "check-config skips a quiet missing file" '! grep -q absent.conf "$TMPDIR/o12"'
assert "and reports a loud one" 'grep -q ghost.conf "$TMPDIR/o12"'

printf '%s\n' "source-file $D/a*.conf" >"$TMPDIR/glob.conf"
printf '%s\n' 'set -g nonsense-option 1' >"$D/a9.conf"
$BIN check-config "$TMPDIR/glob.conf" >"$TMPDIR/o13" 2>&1
assert "check-config reads every file a pattern names" 'grep -q "nonsense-option" "$TMPDIR/o13"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
