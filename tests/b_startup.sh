#!/bin/bash
# Four clients start at the same moment under `script` (the first one to get there starts the
# server), over and over. Every round waits on real markers, not sleeps: the socket must appear
# and `imux ls` must list all four sessions. A client that never starts shows up as a failed
# round with the time it took, so a startup stall is told apart from a slow test.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-startup.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
ROUNDS="${ROUNDS:-10}"
LIMIT="${LIMIT:-20}"
fail=0
slowest=0
SOCK="$TMPDIR/imux-$(id -u)/default"

(sleep 600; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &
watchdog=$!

start_client() {
  local n=$1 round=$2
  (sleep 6) | timeout 30 script -q "$TMPDIR/r$round-c$n.out" sh -c "stty rows 30 cols 100; $BIN new -s s$n" >/dev/null 2>&1 &
}

round=1
while [ "$round" -le "$ROUNDS" ]; do
  t0=$(date +%s)
  pids=""
  for n in 1 2 3 4; do start_client "$n" "$round"; pids="$pids $!"; done
  seen=0
  elapsed=0
  while [ "$elapsed" -le "$LIMIT" ]; do
    seen=$($BIN ls 2>/dev/null | grep -c '^s[1-4]:')
    [ "$seen" = 4 ] && break
    sleep 0.25
    elapsed=$(($(date +%s) - t0))
  done
  elapsed=$(($(date +%s) - t0))
  [ "$elapsed" -gt "$slowest" ] && slowest=$elapsed
  if [ "$seen" = 4 ]; then
    echo "ok   round $round: four clients up in ${elapsed}s"
  else
    echo "FAIL round $round: only $seen of 4 sessions after ${elapsed}s (socket: $([ -S "$SOCK" ] && echo present || echo missing))"
    fail=1
    for n in 1 2 3 4; do
      f="$TMPDIR/r$round-c$n.out"
      printf '       client %s typescript: %s bytes\n' "$n" "$(wc -c <"$f" 2>/dev/null || echo 0)"
    done
    break
  fi
  $BIN kill-server >/dev/null 2>&1
  wait $pids 2>/dev/null
  round=$((round + 1))
done

echo "slowest round: ${slowest}s"
$BIN kill-server >/dev/null 2>&1
kill "$watchdog" 2>/dev/null
wait 2>/dev/null
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
