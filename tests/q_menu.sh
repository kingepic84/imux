#!/bin/bash
# `display-menu`: a binding that opens a menu, choosing by shortcut, by the movement keys and Enter,
# by a click, closing by Escape and by a click outside, a disabled item, what the box shows, the
# parse errors, and the refusal from inside a pane.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qm.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cat >"$IMUX_CONFIG" <<'EOF'
bind m display-menu -T "Pick one" "Alpha" a "rename-window alpha" "" "Beta" b "rename-window beta" "-Gone" g "rename-window gone"
EOF

$BIN new-session -d -s mnu -n start >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "the binding with a menu loaded" 'grep -aq "display-menu" "$TMPDIR/keys"'

name() { $BIN list-windows -F '#{window_name}' 2>/dev/null | head -1; }

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 25 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t mnu" >/dev/null 2>&1
}

keys_a() { printf '\002m'; sleep 0.7; printf 'a'; }
client keys_a
assert "a shortcut chooses its item" '[ "$(name)" = alpha ]'
assert "the box shows the title, the names and the keys" 'LC_ALL=C grep -aq "Pick one" "$TMPDIR/c.out" && LC_ALL=C grep -aq "Beta" "$TMPDIR/c.out" && LC_ALL=C grep -aq "(b)" "$TMPDIR/c.out"'

keys_nav() { printf '\002m'; sleep 0.7; printf 'j'; sleep 0.3; printf '\r'; }
client keys_nav
assert "the movement keys skip the rule and Enter chooses" '[ "$(name)" = beta ]'

$BIN rename-window keep >/dev/null 2>&1
keys_disabled() { printf '\002m'; sleep 0.7; printf 'g'; sleep 0.5; printf '\033'; sleep 0.7; printf '\002m'; sleep 0.7; printf 'a'; }
client keys_disabled
assert "a disabled item does not run, Escape closes the menu, and a new one opens after it" '[ "$(name)" = alpha ]'

$BIN rename-window keep >/dev/null 2>&1
keys_q() { printf '\002m'; sleep 0.7; printf 'q'; sleep 0.5; printf 'x'; }
client keys_q
assert "q closes it and nothing ran" '[ "$(name)" = keep ]'

keys_click() { printf '\002m'; sleep 0.7; printf '\033[<0;47;15M\033[<0;47;15m'; }
client keys_click
assert "a click on an item chooses it" '[ "$(name)" = beta ]'

$BIN rename-window keep >/dev/null 2>&1
keys_outside() { printf '\002m'; sleep 0.7; printf '\033[<0;2;2M\033[<0;2;2m'; sleep 0.5; printf 'a'; }
client keys_outside
assert "a click outside closes it and the key after it is not a shortcut" '[ "$(name)" = keep ]'

$BIN display-menu -T x a >"$TMPDIR/e1" 2>&1
assert "an item without its key and command is an error" 'grep -aq "not enough arguments" "$TMPDIR/e1"'
$BIN display-menu -b wavy a b c >"$TMPDIR/e2" 2>&1
assert "an unknown border is an error" 'grep -aq "display-menu: -b is one of" "$TMPDIR/e2"'
$BIN display-menu -s nonsense a b c >"$TMPDIR/e3" 2>&1
assert "a bad style is an error" 'grep -aq "display-menu: -s" "$TMPDIR/e3"'
$BIN display-menu -z a b c >"$TMPDIR/e4" 2>&1
assert "an unknown flag is an error" 'grep -aq "unknown flag -z" "$TMPDIR/e4"'
$BIN display-menu a b c >"$TMPDIR/e5" 2>&1
assert "outside a pane with no client attached it says so" 'grep -aq "no client is attached" "$TMPDIR/e5"'

$BIN send-keys -t %0 C-u >/dev/null 2>&1
$BIN send-keys -t %0 -l "$BIN display-menu a b c 2>&1 | grep -c 'not allowed' | sed 's/^/COUNT:/'" >/dev/null 2>&1
$BIN send-keys -t %0 Enter >/dev/null 2>&1
sleep 2
assert "from inside a pane it is refused" '$BIN capture-pane -p -t %0 | grep -aq "COUNT:[1-9]"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
