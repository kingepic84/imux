#!/bin/bash
# paste-buffer -p: taken as a flag, and a paste into a pane that asked for bracketed paste carries
# the markers (cat -v shows them as ^[[200~ and ^[[201~); into a pane that did not, it does not.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qp.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

paste() {
  (sleep 2; printf '\002:'; sleep 0.5; printf '%s\r' "$1"; sleep 2; printf '\002d'; sleep 0.5) \
    | timeout 30 script -q "$TMPDIR/p.out" sh -c "stty rows 30 cols 100; $BIN attach -t $2" >/dev/null 2>&1
}

$BIN new-session -d -s on -n one >/dev/null 2>&1
$BIN new-session -d -s off -n one >/dev/null 2>&1
$BIN send-keys -t on:one "printf '\\033[?2004h'; cat -v" Enter >/dev/null 2>&1
$BIN send-keys -t off:one 'cat -v' Enter >/dev/null 2>&1
sleep 1

paste 'set-buffer -b mine hello ; paste-buffer -p -b mine' on
$BIN capture-pane -p -t on:one >"$TMPDIR/on.txt" 2>&1
assert "a pane that asked gets the markers" 'grep -aqF "^[[200~hello" "$TMPDIR/on.txt" && grep -aqF "^[[201~" "$TMPDIR/on.txt"'

paste 'set-buffer -b mine hello ; paste-buffer -p -b mine' off
$BIN capture-pane -p -t off:one >"$TMPDIR/off.txt" 2>&1
assert "a pane that did not gets the text alone" 'grep -aqF "hello" "$TMPDIR/off.txt" && ! grep -aqF "200~" "$TMPDIR/off.txt"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
