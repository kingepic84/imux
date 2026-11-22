#!/bin/bash
# `display-popup`: opening from a key, what the box shows, keys going to the program, -E closing
# at the end of the program, a popup that stays until Escape, -EE and the status, the working
# directory and variables, one popup per window, -C, the three-prefix escape hatch, a command on
# the socket that waits for the popup and returns its status, a popup asked for by a program in
# a pane (origin marker, popup-from-panes), and the plugin and parse errors.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qp.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 170; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cat >"$IMUX_CONFIG" <<EOF
bind p display-popup -E -T "pop" -w 40 -h 8 "echo POPUP-OUT; sleep 30"
bind c display-popup -E "cat > $TMPDIR/typed"
bind f display-popup -E "echo FAST"
bind s display-popup "echo STAYED"
bind x display-popup -E -E "exit 3"
bind y display-popup -E -E "true"
bind e display-popup -E -d /tmp -e FOO=bar "sh -c 'env > $TMPDIR/env.out; pwd > $TMPDIR/pwd.out'"
bind w display-popup -E -T tt "sleep 30"
EOF

$BIN new-session -d -s pop >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "the bindings with a popup loaded" 'grep -aq "display-popup" "$TMPDIR/keys"'

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 40 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t pop" >/dev/null 2>&1
}

shows() { $BIN capture-pane -p -t %0 | grep -aqx "$1"; }

k_open() { printf '\002p'; sleep 1; }
client k_open
assert "a popup shows its title and what its program printed" 'LC_ALL=C grep -aq "pop" "$TMPDIR/c.out" && LC_ALL=C grep -aq "POPUP-OUT" "$TMPDIR/c.out"'

k_type() { printf '\002c'; sleep 0.8; printf 'hello\r'; sleep 0.5; printf '\004'; sleep 0.8; printf 'echo ZZTYPED\r'; sleep 0.8; }
client k_type
assert "typed keys go to the program of the popup" 'grep -aq hello "$TMPDIR/typed"'
assert "they do not reach the pane" '! shows hello'
assert "the end of the program closes a popup with -E and the keys go back to the pane" 'shows ZZTYPED'

k_fast() { printf '\002f'; sleep 1.2; printf 'echo ZZFAST\r'; sleep 0.8; }
client k_fast
assert "a popup whose program is over at once is gone" 'shows ZZFAST'

k_stay() { printf '\002s'; sleep 1; printf 'echo ZZGONE\r'; sleep 0.6; printf '\033'; sleep 0.8; printf 'echo ZZBACK\r'; sleep 0.8; }
client k_stay
assert "without -E the popup stays after its program, showing what it printed" 'LC_ALL=C grep -aq "STAYED" "$TMPDIR/c.out"'
assert "a key that is not Escape is dropped while it stays" '! shows ZZGONE'
assert "Escape closes it" 'shows ZZBACK'

k_fail() { printf '\002x'; sleep 1; printf 'echo ZZFAIL\r'; sleep 0.6; printf '\033'; sleep 0.8; printf 'echo ZZOK\r'; sleep 0.8; }
client k_fail
assert "-E -E keeps a popup whose program failed" '! shows ZZFAIL'
assert "and Escape closes it" 'shows ZZOK'

k_ok() { printf '\002y'; sleep 1.2; printf 'echo ZZSUCCESS\r'; sleep 0.8; }
client k_ok
assert "-E -E closes a popup whose program succeeded" 'shows ZZSUCCESS'

k_env() { printf '\002e'; sleep 1.5; }
client k_env
assert "-e is a variable of the program and -d its directory" 'grep -aq "^FOO=bar$" "$TMPDIR/env.out" && grep -aq "tmp" "$TMPDIR/pwd.out"'

k_second() {
  printf '\002w'; sleep 1
  timeout 8 $BIN display-popup -E "echo SECONDX" >"$TMPDIR/second.out" 2>&1
  echo $? >"$TMPDIR/second.rc"
  printf '\002\002'; sleep 0.4; printf 'echo ZZSTILL\r'; sleep 0.4
  printf '\002\002\002'; sleep 0.8
  printf 'echo ZZCHORD\r'; sleep 0.8
}
client k_second
assert "a second popup in the window is ignored and answered at once" '[ "$(cat "$TMPDIR/second.rc")" = 0 ] && ! LC_ALL=C grep -aq SECONDX "$TMPDIR/c.out"'
assert "two presses of the prefix do not close it" '! shows ZZSTILL'
assert "three presses of the prefix close it whatever the program does" 'shows ZZCHORD'

k_close() { printf '\002w'; sleep 1; $BIN display-popup -C >/dev/null 2>&1; sleep 0.5; printf 'echo ZZCLOSED\r'; sleep 0.8; }
client k_close
assert "display-popup -C closes it" 'shows ZZCLOSED'

k_wait() {
  timeout 8 $BIN display-popup -E "sh -c 'exit 7'" >"$TMPDIR/w7.out" 2>&1
  echo $? >"$TMPDIR/w7.rc"
  timeout 8 $BIN display-popup -E "sleep 0.5" >"$TMPDIR/w0.out" 2>&1
  echo $? >"$TMPDIR/w0.rc"
}
client k_wait
assert "a command on the socket waits and fails with the status of the program" '[ "$(cat "$TMPDIR/w7.rc")" = 1 ] && grep -aq "status 7" "$TMPDIR/w7.out"'
assert "and succeeds when the program does" '[ "$(cat "$TMPDIR/w0.rc")" = 0 ]'

k_pane() {
  printf '%s\r' "$BIN display-popup -E -T tt 'sleep 1.5'; echo ZZPANEDONE"
  sleep 1
}
client k_pane
assert "a popup asked for by a program in a pane shows where it came from" 'LC_ALL=C grep -aq "\[from %0\]" "$TMPDIR/c.out"'
assert "and the program that asked waited for it" 'shows ZZPANEDONE'

$BIN set -g popup-from-panes off >/dev/null 2>&1
$BIN send-keys -t %0 -l "$BIN display-popup -E x 2>&1 | grep -c 'popup-from-panes' | sed 's/^/PFP:/'" >/dev/null 2>&1
$BIN send-keys -t %0 Enter >/dev/null 2>&1
sleep 2
assert "with popup-from-panes off a pane is refused and told which option" '$BIN capture-pane -p -t %0 | grep -aq "PFP:[1-9]"'

$BIN display-popup -b wavy x >"$TMPDIR/e1" 2>&1
assert "an unknown border is an error" 'grep -aq "display-popup: -b is one of" "$TMPDIR/e1"'
$BIN display-popup -w 0 x >"$TMPDIR/e2" 2>&1
assert "a size of zero is an error" 'grep -aq "display-popup: -w" "$TMPDIR/e2"'
$BIN display-popup x >"$TMPDIR/e3" 2>&1
assert "with no client attached it says so" 'grep -aq "no client is attached" "$TMPDIR/e3"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
