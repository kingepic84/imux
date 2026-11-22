#!/bin/bash
# autosave-history: timestamped rolling copies of the autosave, kept to a count, listed by
# `imux snapshots --history` and brought back by `imux restore history/NAME`; never restored by
# themselves; off at 0.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qh.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
H="$IMUX_STATE_DIR/snapshots/history"

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

until_true() {
  local n=0
  while [ "$n" -lt "$2" ]; do
    eval "$1" && return 0
    sleep 0.25
    n=$((n + 1))
  done
  return 1
}

count() {
  ls "$H" 2>/dev/null | grep -c '\.imux$'
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set autosave-interval 1\nset autosave-history-interval 1\nset autosave-history 2\n' >"$IMUX_CONFIG"
$BIN kill-server >/dev/null 2>&1
sleep 0.5
$BIN new-session -d -s alpha >/dev/null 2>&1
until_true '[ "$(count)" -ge 1 ]' 40
assert "a first rolling copy appears" '[ "$(count)" -ge 1 ]'
FIRST=$(ls "$H" | grep '\.imux$' | head -1)
assert "its file is private" '[ "$(stat -f %Lp "$H/$FIRST" 2>/dev/null || stat -c %a "$H/$FIRST")" = 600 ]'

for n in w1 w2 w3; do
  sleep 1.6
  $BIN new-window -t alpha: -n "$n" >/dev/null 2>&1
  until_true "ls \"$H\" | grep -q ." 8
done
sleep 2.5
assert "the count limits the copies" '[ "$(count)" = 2 ]'
assert "imux snapshots --history lists them" '[ "$($BIN snapshots --history | grep -c "^history/")" = 2 ]'
assert "the newest copy shows the session" '$BIN snapshots --history | head -1 | grep -q "alpha"'
NEWEST=$($BIN snapshots --history | head -1 | sed 's/ .*//')
assert "the name is history/NAME" 'echo "$NEWEST" | grep -q "^history/[0-9][0-9]*$"'

$BIN kill-server >/dev/null 2>&1
sleep 0.5
assert "kill-server leaves the rolling copies" '[ "$(count)" = 2 ]'
assert "and no autosave" '[ ! -e "$IMUX_STATE_DIR/autosave.imux" ]'

$BIN new-session -d -s beta >/dev/null 2>&1
$BIN set -g autosave-history 10 >/dev/null 2>&1
sleep 1
assert "a new server does not restore a rolling copy by itself" '! $BIN ls | grep -q "^alpha:"'
$BIN restore "$NEWEST" >"$TMPDIR/r.out" 2>&1
assert "restore history/NAME brings the session back" '$BIN ls | grep -q "^alpha:"'
$BIN restore history/../x >"$TMPDIR/r2.out" 2>&1
assert "a bad name is refused" 'grep -aq "no rolling copy named" "$TMPDIR/r2.out"'
$BIN kill-server >/dev/null 2>&1

echo "-- off at 0"
rm -rf "$IMUX_STATE_DIR"
printf 'set autosave-interval 1\nset autosave-history 0\n' >"$IMUX_CONFIG"
sleep 0.5
$BIN new-session -d -s gamma >/dev/null 2>&1
until_true '[ -s "$IMUX_STATE_DIR/autosave.imux" ]' 40
sleep 1
assert "no rolling copy is made" '[ "$(count)" = 0 ] && [ "$($BIN snapshots --history)" = "no rolling copies" ]'
$BIN kill-server >/dev/null 2>&1

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
