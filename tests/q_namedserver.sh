#!/bin/bash
# Named servers: -L name, -S path and $IMUX_SOCKET_NAME give separate servers with their own
# sockets, logs and state; a pane talks to its own server; the tmux shim honors -L.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qns.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_SOCKET_PATH IMUX_SOCKET_NAME IMUX_SERVER_SOCKET
fail=0
SD="$TMPDIR/imux-$(id -u)"

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

printf 'set autosave-interval 1\n' >"$IMUX_CONFIG"
(sleep 120; $BIN kill-server >/dev/null 2>&1; $BIN -L two kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN kill-server >/dev/null 2>&1
$BIN -L two kill-server >/dev/null 2>&1
sleep 0.5
$BIN new-session -d -s d "$BIN ls >$TMPDIR/d.ls 2>&1; sleep 3; tmux -L two list-sessions >$TMPDIR/d.shim 2>&1; sleep 30" >/dev/null 2>&1
$BIN -L two new-session -d -s t "$BIN ls >$TMPDIR/t.ls 2>&1; sleep 30" >/dev/null 2>&1

assert "the default server lists its own session only" '$BIN ls | grep -q "^d:" && ! $BIN ls | grep -q "^t:"'
assert "the named server lists its own session only" '$BIN -L two ls | grep -q "^t:" && ! $BIN -L two ls | grep -q "^d:"'
assert "two sockets" '[ -S "$SD/default" ] && [ -S "$SD/two" ]'
assert "the named server has its own log" '[ -s "$SD/server-two.log" ] && [ -s "$SD/server.log" ]'

until_true '[ -s "$TMPDIR/t.ls" ] && [ -s "$TMPDIR/d.ls" ] && [ -s "$TMPDIR/d.shim" ]' 40
assert "a pane of the named server reaches its own server" 'grep -q "^t:" "$TMPDIR/t.ls" && ! grep -q "^d:" "$TMPDIR/t.ls"'
assert "a pane of the default server reaches the default server" 'grep -q "^d:" "$TMPDIR/d.ls" && ! grep -q "^t:" "$TMPDIR/d.ls"'
assert "tmux -L from a pane reaches the named server" 'grep -q "^t:" "$TMPDIR/d.shim"'

until_true '[ -s "$IMUX_STATE_DIR/two/autosave.imux" ] && [ -s "$IMUX_STATE_DIR/autosave.imux" ]' 40
assert "each server has its own state" '[ -s "$IMUX_STATE_DIR/two/autosave.imux" ] && [ -s "$IMUX_STATE_DIR/autosave.imux" ]'
assert "and the named one's autosave is its own" 'grep -q "^session t " "$IMUX_STATE_DIR/two/autosave.imux" && ! grep -q "^session d " "$IMUX_STATE_DIR/two/autosave.imux"'

assert "the name comes from the environment too" 'IMUX_SOCKET_NAME=two $BIN ls | grep -q "^t:"'
assert "and a flag beats it" 'IMUX_SOCKET_NAME=two $BIN -L default ls | grep -q "^d:"'
assert "-S names a socket by path" '$BIN -S "$SD/two" ls | grep -q "^t:"'

$BIN -L a/b ls >"$TMPDIR/bad.out" 2>&1
rc=$?
assert "a bad name is refused with status 2" '[ "$rc" = 2 ] && grep -q "server name" "$TMPDIR/bad.out"'
$BIN -L >"$TMPDIR/bad2.out" 2>&1
rc=$?
assert "a flag with no value is refused" '[ "$rc" = 2 ]'

mkdir -m 700 "$TMPDIR/priv"
$BIN -S "$TMPDIR/priv/sock" new-session -d -s p "sleep 30" >/dev/null 2>&1
assert "-S starts a server at a private path" '[ -S "$TMPDIR/priv/sock" ] && $BIN -S "$TMPDIR/priv/sock" ls | grep -q "^p:"'
assert "with its own state" 'until_true "[ -s \"$IMUX_STATE_DIR/sock/autosave.imux\" ]" 40'
$BIN -S "$TMPDIR/priv/sock" kill-server >/dev/null 2>&1

$BIN -L two kill-server >/dev/null 2>&1
assert "kill-server -L ends only that server" '$BIN ls | grep -q "^d:" && ! $BIN -L two ls >/dev/null 2>&1'
assert "and removes its autosave only" '[ ! -e "$IMUX_STATE_DIR/two/autosave.imux" ] && [ -s "$IMUX_STATE_DIR/autosave.imux" ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
