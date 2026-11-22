#!/bin/bash
# word-separators: where the vi `w` motion of copy mode stops. Built in, letters and digits make a
# word and `-` is punctuation; with only a blank as separator the word runs to the blank.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qs.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set copy-command ""\nset mode-keys vi\n' >"$IMUX_CONFIG"
$BIN new-session -d -s s -n one >/dev/null 2>&1
$BIN send-keys -t s:one 'printf "AA-BB CC\\n"' Enter >/dev/null 2>&1
sleep 1.5

yank_word() {
  (sleep 1.5; printf '\002['; sleep 0.5; printf '?AA-BB'; sleep 0.3; printf '\r'; sleep 0.5; printf 'v'; sleep 0.3; printf 'w'; sleep 0.3; printf 'y'; sleep 0.8; printf '\002:'; sleep 0.3; printf "save-buffer $TMPDIR/buf\\r"; sleep 0.8; printf '\002d'; sleep 0.5) \
    | timeout 30 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t s" >/dev/null 2>&1
}

yank_word
assert "built in, w stops after the hyphen" '[ "$(head -c 3 "$TMPDIR/buf")" = "AA-" ] && [ "$(wc -c <"$TMPDIR/buf" | tr -d " ")" = 3 ]'

$BIN set -g word-separators ' ' >/dev/null 2>&1
rm -f "$TMPDIR/buf"
yank_word
assert "with a blank as the only separator, w goes to the next word" '[ "$(cat "$TMPDIR/buf")" = "AA-BB C" ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
