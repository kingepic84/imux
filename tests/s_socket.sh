#!/bin/bash
# Phase 7 (S4/S7): a client that speaks nonsense, floods or hoards connections must not hurt the
# server or the other clients.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-ssock.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
SOCKDIR="$TMPDIR/imux-$(id -u)"
SOCK="$SOCKDIR/default"
LOG="$SOCKDIR/server.log"

printf 'set autosave-interval 0\n' >"$IMUX_CONFIG"

ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

(sleep 120; $BIN kill-server >/dev/null 2>&1) &

alive() { timeout 10 $BIN ls 2>&1 | grep -q '^s:'; }

# send HEX... as raw bytes to the socket, then close
send_bytes() {
  perl -e 'use IO::Socket::UNIX; my $s = IO::Socket::UNIX->new(Peer => shift) or die "connect: $!"; print $s pack("H*", join("", @ARGV)); close $s;' "$SOCK" "$@"
}

$BIN new-session -d -s s || { echo "FAIL cannot start a session"; exit 1; }
alive && ok "the server answers" || bad "the server does not answer at the start"

echo "-- malformed frames"
send_bytes 00000000;                    sleep 0.3; alive && ok "frame of length 0" || bad "length 0"
send_bytes ffffffff01;                  sleep 0.3; alive && ok "frame of length 4 GiB" || bad "length 4 GiB"
send_bytes 0000000163;                  sleep 0.3; alive && ok "unknown tag" || bad "unknown tag"
send_bytes 00000002010a;                sleep 0.3; alive && ok "truncated hello payload" || bad "truncated hello"
send_bytes 00000005 02 41414141;        sleep 0.3; alive && ok "odd payload for a known tag" || bad "odd payload"
send_bytes 0000000901ffffffff00000000;  sleep 0.3; alive && ok "hello with a 65535 x 65535 terminal" || bad "huge terminal"
grep -aq 'terminal size too large' "$LOG" && ok "the huge size is refused and logged" || bad "huge size not refused"
perl -e 'use IO::Socket::UNIX; my $s = IO::Socket::UNIX->new(Peer => shift) or die; my $junk = join("", map { chr(int(rand(256))) } 1..65536); for (1..16) { print $s $junk; } close $s;' "$SOCK" 2>/dev/null
sleep 0.5; alive && ok "1 MiB of random bytes" || bad "random bytes"
grep -aq 'protocol error' "$LOG" && ok "protocol errors are logged" || bad "no protocol error in the log"

echo "-- a frame that announces 16 MiB and stops"
perl -e 'use IO::Socket::UNIX; my $s = IO::Socket::UNIX->new(Peer => shift) or die; print $s pack("N", 16*1024*1024); print $s "A" x (4*1024*1024); sleep 3; close $s;' "$SOCK" &
HOLD=$!
sleep 1
alive && ok "other clients are served meanwhile" || bad "server stuck behind a half frame"
wait "$HOLD" 2>/dev/null

echo "-- more connections than the limit"
perl -e 'use IO::Socket::UNIX; my $path = shift; my @s; for (1..320) { my $c = IO::Socket::UNIX->new(Peer => $path); push @s, $c if $c; } sleep 4;' "$SOCK" &
MANY=$!
sleep 2
REFUSED=$(grep -ac 'connection refused: 256 clients' "$LOG")
[ "$REFUSED" -ge 1 ] && ok "connections past the limit were refused ($REFUSED)" || bad "no connection was refused"
wait "$MANY" 2>/dev/null
sleep 1
alive && ok "the server answers after the connections are gone" || bad "server does not answer after the flood"

echo "-- a client that never reads"
(perl -e 'use IO::Socket::UNIX; my $s = IO::Socket::UNIX->new(Peer => shift) or die; print $s pack("N", 1) . "\x09"; sleep 6;' "$SOCK" >/dev/null 2>&1) &
for i in $(seq 1 200); do $BIN rename-window -t s "n$i" >/dev/null 2>&1; done
alive && ok "renames go on while one client does not read" || bad "server stuck behind a client that does not read"

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
