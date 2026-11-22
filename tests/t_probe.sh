#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-probe.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
fail=0
ESC=$(printf '\033')
QUERY="${ESC}_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA${ESC}\\${ESC}[c"

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aqF -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

run_client() {
  (sleep 2; printf 'echo up-$((1+2))\r'; sleep 1.5; printf '\002d'; sleep 1) \
    | timeout 40 script -q "$1" sh -c "stty rows 30 cols 100; env -u LC_TERMINAL -u COLORTERM -u KITTY_WINDOW_ID TERM_PROGRAM=iTerm.app $BIN new -s p" >/dev/null
}

(sleep 60; $BIN kill-server >/dev/null 2>&1) &

printf 'set image-probe-timeout 0\n' >"$IMUX_CONFIG"
run_client "$TMPDIR/off.out"
$BIN kill-server >/dev/null 2>&1
sleep 1
printf '' >"$IMUX_CONFIG"
run_client "$TMPDIR/on.out"
$BIN kill-server >/dev/null 2>&1

check "by default the terminal is asked what it supports" "$QUERY" "$TMPDIR/on.out"
refute "with image-probe-timeout 0 it is not asked" "${ESC}_Gi=31" "$TMPDIR/off.out"
check "and the session works as before" "up-3" "$TMPDIR/off.out"

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
