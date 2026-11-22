#!/bin/bash
# A lone brace that is the key of `bind` or `unbind` (`unbind {`, `bind } swap-pane -D`) is a key and
# not a block brace: it is read as a key, and the lines after it still count. A brace after the key
# opens a block as before, and a stray one is still refused.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qbk.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

printf 'unbind }\nunbind {\nbind } swap-pane -D\nbind { swap-pane -U\nbind -T prefix x { new-window }\nset -g nosuchoption 1\n' >"$TMPDIR/keys.conf"
$BIN check-config "$TMPDIR/keys.conf" >"$TMPDIR/keys.out" 2>&1
assert "the braces are keys: nothing is refused" '! grep -aq "refused" "$TMPDIR/keys.out"'
assert "the line after them is read, with its own number" 'grep -aq "keys.conf:6: unknown-option: nosuchoption" "$TMPDIR/keys.out"'
assert "all six lines are counted" 'grep -aq "^files: 1, lines: 6, problems: 1$" "$TMPDIR/keys.out"'

printf 'bind x new-window }\nset -g nosuchoption 1\n' >"$TMPDIR/stray.conf"
$BIN check-config "$TMPDIR/stray.conf" >"$TMPDIR/stray.out" 2>&1
assert "a stray closing brace after the key is still refused" 'grep -aq "stray.conf:1: refused: unexpected } with no { block open" "$TMPDIR/stray.out"'
assert "and the line after it is still read" 'grep -aq "stray.conf:2: unknown-option: nosuchoption" "$TMPDIR/stray.out"'

printf 'display {\nset -g nosuchoption 1\n' >"$TMPDIR/open.conf"
$BIN check-config "$TMPDIR/open.conf" >"$TMPDIR/open.out" 2>&1
assert "a brace after a command that takes no key still opens a block" 'grep -aq "open.conf:1: refused: a { block has no matching }" "$TMPDIR/open.out"'

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
