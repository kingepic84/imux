#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-copykeys.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 200; $BIN kill-server >/dev/null 2>&1) &

echo "-- emacs keys: search, select, copy, paste"
printf 'set copy-command ""\nset mode-keys emacs\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=1 client "$TMPDIR/a.out" \
  'echo copy-me-$((6*7))\r' \
  '\002[' \
  '\022' 'copy-me-4' '\r' \
  '\000' '\005' '\033w' \
  'echo got:' '\002]' '\r' >/dev/null
check "C-r finds the text, C-Space and C-e select to the end of the line, M-w copies" 'got:copy-me-42' "$TMPDIR/a.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- a binding in the copy mode table"
printf 'set copy-command ""\nset mode-keys vi\nbind -T copy-mode-vi Z send -X cancel\nunbind -T copy-mode-vi q\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=1 client "$TMPDIR/b.out" \
  '\002[' 'Z' \
  'echo bound-$((1+2))\r' \
  '\002[' 'q' \
  'echo unbound-$((1+2))\r' >/dev/null
check "a key bound with bind -T copy-mode-vi works" 'bound-3' "$TMPDIR/b.out"
assert "an unbound key does nothing (the mode is still on, the typed line is not run)" '! grep -aq "unbound-3" "$TMPDIR/b.out"'
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- list-keys shows the copy mode tables"
printf 'set copy-command ""\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=4 client "$TMPDIR/c.out" 'echo up\r' >/dev/null &
CLIENT=$!
sleep 3
$BIN list-keys >"$TMPDIR/keys.txt" 2>&1
check "the vi table is listed" '^bind-key -T copy-mode-vi y +send-keys -X copy-selection-and-cancel$' "$TMPDIR/keys.txt"
check "and the emacs table" '^bind-key -T copy-mode C-Space +send-keys -X begin-selection$' "$TMPDIR/keys.txt"
wait $CLIENT
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- mode-keys follows the editor when the configuration does not say"
printf 'set copy-command ""\n' >"$IMUX_CONFIG"
(
  unset VISUAL
  export EDITOR=vim
  ARGS="new -s w" HOLD=1 client "$TMPDIR/d.out" '\002[' 'y' 'echo ed-$((1+2))\r' >/dev/null
)
check "with vim as the editor the vi keys are on (y leaves)" 'ed-3' "$TMPDIR/d.out"
$BIN kill-server >/dev/null 2>&1
sleep 1
(
  unset VISUAL
  export EDITOR=nano
  ARGS="new -s w" HOLD=1 client "$TMPDIR/e.out" '\002[' 'y' 'echo ed-$((1+2))\r' >/dev/null
)
assert "with another editor the emacs keys are on (y does nothing, the mode stays)" '! grep -aq "ed-3" "$TMPDIR/e.out"'
$BIN kill-server >/dev/null 2>&1
sleep 1

grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
