#!/bin/bash
# Formats outside `-F` know what `-F` formats know: a display-message bound to a key, and menu and
# popup titles, read pane and window variables, `@` options, option names and environment names.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qfl.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

keys() {
  (sleep 2; printf "$1"; sleep 1.5; printf '\033'; sleep 0.5; printf '\002d'; sleep 0.5) \
    | timeout 30 script -q "$TMPDIR/k.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1
}

$BIN new-session -d -s s -n one >/dev/null 2>&1
$BIN set -g @mark hello >/dev/null 2>&1
$BIN set-environment -g FMTLOOKUP_TEST envvalue >/dev/null 2>&1
$BIN bind-key x display-message 'M[#{window_width}|#{@mark}|#{status-position}|#{FMTLOOKUP_TEST}|#{session_name}]' >/dev/null 2>&1
$BIN bind-key y display-menu -T 'T[#{@mark}|#{status-position}|#{FMTLOOKUP_TEST}]' 'one' o 'display-message hi' >/dev/null 2>&1

keys '\002x'
assert "a display-message on a key reads window variables, a user option, an option name, the environment" 'grep -aqE "M\[[0-9]+\|hello\|bottom\|envvalue\|s\]" "$TMPDIR/k.out"'

keys '\002y'
assert "a menu title reads a user option, an option name and the environment" 'grep -aqF "T[hello|bottom|envvalue]" "$TMPDIR/k.out"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
