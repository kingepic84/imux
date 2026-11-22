#!/bin/bash
# P10.5, `imux pane wait-output PANE (--match TEXT | --regex RE) [--source recent|visible]
# [--lines N] [--timeout MS] [--json]`: a line that is already there, a line that comes later,
# the timeout, a pattern that does not compile, a pane that goes away or whose program ended, the
# limit of waits for one caller, a client that leaves, the two sources, escape sequences left out,
# and the rules for a program in a pane.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qpw.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
fail=0
seq=0
bgn=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 300; $BIN kill-server >/dev/null 2>&1; kill $$ >/dev/null 2>&1) >/dev/null 2>&1 &

printf 'set -g status off\nset -g default-size 100x30\n' >"$IMUX_CONFIG"
HOLDER="$(cd "$(dirname "$0")" && pwd)/fixtures/pane_holder.sh"

run() {
  OUT="$TMPDIR/out"
  ERR="$TMPDIR/err"
  timeout 30 $BIN "$@" >"$OUT" 2>"$ERR" </dev/null
  RC=$?
}

inpane() {
  shift
  seq=$((seq + 1))
  rm -f "$TMPDIR/rc$seq"
  (trap '' PIPE; printf '%s\n' "{ $*; } >$TMPDIR/o$seq 2>&1; echo \$? >$TMPDIR/rc$seq" >"$TMPDIR/fifo") >/dev/null 2>&1 &
  local i
  for i in $(seq 1 50); do
    [ -f "$TMPDIR/rc$seq" ] && break
    sleep 0.2
  done
  O="$TMPDIR/o$seq"
  RC=$(cat "$TMPDIR/rc$seq" 2>/dev/null)
  [ -f "$O" ] || : >"$O"
}

bgwait() {
  bgn=$((bgn + 1))
  BG="$TMPDIR/bg$bgn"
  rm -f "$BG" "$BG.rc"
  ( timeout 40 $BIN pane wait-output "$@" >"$BG" 2>&1 </dev/null; echo $? >"$BG.rc" ) &
  BGPID=$!
}

done_within() {
  local i
  for i in $(seq 1 $(($2 * 5))); do
    [ -f "$1.rc" ] && return 0
    sleep 0.2
  done
  return 1
}

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s pw >/dev/null 2>&1
sleep 1
mkfifo "$TMPDIR/fifo"
$BIN send-keys -t %0 -l "sh $HOLDER $TMPDIR/fifo" >/dev/null 2>&1
$BIN send-keys -t %0 Enter >/dev/null 2>&1
for i in $(seq 1 50); do
  [ -f "$TMPDIR/ready" ] && break
  (trap '' PIPE; printf '%s\n' "echo >$TMPDIR/ready" >"$TMPDIR/fifo") >/dev/null 2>&1 &
  sleep 0.2
done
$BIN split-window -d >/dev/null 2>&1
sleep 1

echo "-- a line that is already there"
$BIN pane run 1 'echo EARLY-$((40+2))' >/dev/null 2>&1
sleep 1
run pane wait-output 1 --match EARLY-42 --timeout 3000
assert "it answers at once with the line" '[ $RC = 0 ] && grep -aqx "EARLY-42" "$OUT"'
run pane wait-output 1 --regex '^EARLY-4[0-9]$' --timeout 3000
assert "a regular expression matches the line too" '[ $RC = 0 ] && grep -aqx "EARLY-42" "$OUT"'
run pane wait-output 1 --match EARLY-42 --json
assert "--json has the pane, the line and what was searched" '[ $RC = 0 ] && grep -aq "^{\"pane\":\"%1\",\"line\":\"EARLY-42\",\"text\":\"" "$OUT" && [ "$(wc -l <"$OUT" | tr -d " ")" = 1 ]'

echo "-- a line that comes later"
bgwait 1 --match LATE-42
sleep 1
assert "the wait is still open before the output" '[ ! -f "$BG.rc" ]'
$BIN pane run 1 'echo LATE-$((40+2))' >/dev/null 2>&1
assert "it ends when the line is printed" 'done_within "$BG" 10 && [ "$(cat "$BG.rc")" = 0 ] && grep -aqx "LATE-42" "$BG"'
bgwait 1 --regex '^LATER-[0-9]+$'
sleep 0.5
$BIN pane run 1 'echo LATER-$((1+1))' >/dev/null 2>&1
assert "a regular expression waits for its line" 'done_within "$BG" 10 && grep -aqx "LATER-2" "$BG"'
$BIN pane run 1 "printf '\\033[31mCOLOR-ME\\033[0m\\n'" >/dev/null 2>&1
sleep 0.7
run pane wait-output 1 --regex '^COLOR-ME$' --timeout 3000
assert "escape sequences are not part of the text" '[ $RC = 0 ] && ! LC_ALL=C grep -aq "$(printf "\033")" "$OUT"'

echo "-- the timeout and the refusals"
start=$(date +%s)
run pane wait-output 1 --match NEVER-APPEARS-ZZ --timeout 700 --json
took=$(($(date +%s) - start))
assert "a wait that finds nothing is timeout, exit 1, nothing on stdout" '[ $RC = 1 ] && [ ! -s "$OUT" ] && grep -aq "\"code\":\"timeout\"" "$ERR"'
assert "and ends near the timeout" '[ $took -le 5 ]'
run pane wait-output 1 --regex '(' --json
assert "a pattern that does not compile is invalid_regex" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_regex\"" "$ERR"'
LONG=$(head -c 600 /dev/zero | tr '\0' a)
run pane wait-output 1 --regex "$LONG" --json
assert "a pattern over 512 bytes is invalid_regex" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_regex\"" "$ERR"'
run pane wait-output 1 --match "$LONG" --json
assert "so is text over 512 bytes, as invalid_argument" '[ $RC = 1 ] && grep -aq "\"code\":\"invalid_argument\"" "$ERR"'
run pane wait-output 99 --match x --json
assert "a missing pane is not_found" '[ $RC = 1 ] && grep -aq "\"code\":\"not_found\"" "$ERR"'
run pane wait-output 99 --match x
assert "plain: the text without the code" '[ $RC = 1 ] && grep -aqx "imux: no pane 99" "$OUT"'
run pane wait-output 1
assert "no pattern is a usage error, exit 2" '[ $RC = 2 ] && grep -aq "usage: imux pane wait-output pane" "$OUT"'
run pane wait-output 1 --match a --regex b
assert "both patterns are a usage error, exit 2" '[ $RC = 2 ]'
run pane wait-output 1 --match a --lines 0
assert "--lines 0 is a usage error, exit 2" '[ $RC = 2 ]'

echo "-- the two sources"
$BIN pane run 1 'i=0; while [ $i -lt 60 ]; do echo SCROLL-$i; i=$((i+1)); done' >/dev/null 2>&1
sleep 1.5
run pane wait-output 1 --regex '^SCROLL-2$' --source visible --timeout 600 --json
assert "a line that scrolled off the screen is not visible" '[ $RC = 1 ] && grep -aq "\"code\":\"timeout\"" "$ERR"'
run pane wait-output 1 --regex '^SCROLL-2$' --source recent --lines 100 --timeout 3000
assert "recent looks above the screen" '[ $RC = 0 ] && grep -aqx "SCROLL-2" "$OUT"'
run pane wait-output 1 --regex '^SCROLL-2$' --source recent --lines 5 --timeout 600
assert "but only as far as --lines says" '[ $RC = 1 ]'
run pane wait-output 1 --regex '^SCROLL-59$' --source visible --timeout 3000
assert "the last line is on the screen" '[ $RC = 0 ]'

echo "-- a pane that goes away"
$BIN split-window -d -P -F '#{pane_id}' "sleep 2" >"$TMPDIR/p-gone" 2>&1
GONE=$(grep -a '^%' "$TMPDIR/p-gone" | head -1 | tr -d '%')
sleep 0.5
run pane wait-output "$GONE" --match NEVER-APPEARS-ZZ --timeout 20000 --json
assert "a pane that closes ends the wait with not_found" '[ $RC = 1 ] && grep -aq "\"code\":\"not_found\"" "$ERR" && grep -aq "closed" "$ERR"'
$BIN set -g remain-on-exit on >/dev/null 2>&1
$BIN split-window -d -P -F '#{pane_id}' "sh -c 'echo BYE-$((6*7)); exit 0'" >"$TMPDIR/p-dead" 2>&1
DEAD=$(grep -a '^%' "$TMPDIR/p-dead" | head -1 | tr -d '%')
sleep 1.5
run pane wait-output "$DEAD" --match NEVER-APPEARS-ZZ --timeout 20000 --json
assert "a pane whose program ended is not_ready" '[ $RC = 1 ] && grep -aq "\"code\":\"not_ready\"" "$ERR"'
run pane wait-output "$DEAD" --match "BYE-" --timeout 3000
assert "but what it printed can still be found" '[ $RC = 0 ] && grep -aq "BYE-" "$OUT"'
$BIN set -g remain-on-exit off >/dev/null 2>&1

echo "-- limits, and a client that leaves"
pids=()
for n in 1 2 3 4 5 6 7 8; do
  timeout 40 $BIN pane wait-output 1 --match "NEVER-APPEARS-$n" >/dev/null 2>&1 </dev/null &
  pids+=("$!")
done
sleep 1
run pane wait-output 1 --match NEVER-APPEARS-9 --json
assert "a ninth wait of the same caller is limit_reached" '[ $RC = 1 ] && grep -aq "\"code\":\"limit_reached\"" "$ERR"'
for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
sleep 1.5
run pane wait-output 1 --match NEVER-APPEARS-10 --timeout 600 --json
assert "after the clients left their waits are gone" '[ $RC = 1 ] && grep -aq "\"code\":\"timeout\"" "$ERR"'

echo "-- from a pane"
inpane 0 "$BIN new-window -d -P -F '#{pane_id}'"
KID=$(grep -a '^%' "$O" | head -1 | tr -d '%')
assert "the pane in the holder spawned a pane" '[ -n "$KID" ]'
sleep 1
$BIN pane run "$KID" 'echo KID-$((6*7))' >/dev/null 2>&1
sleep 1
inpane 0 "$BIN pane wait-output $KID --match KID-42 --timeout 3000"
assert "it may wait on the pane it spawned" '[ "$RC" = 0 ] && grep -aqx "KID-42" "$O"'
inpane 0 "$BIN pane wait-output 1 --match EARLY-42 --timeout 3000"
assert "not on one it did not spawn" '[ "$RC" != 0 ] && grep -aq "not one this pane spawned" "$O"'
inpane 0 "$BIN pane wait-output 0 --match pane_holder --timeout 3000"
assert "its own pane is fine" '[ "$RC" = 0 ]'
$BIN set -g command-from-panes off >/dev/null 2>&1
inpane 0 "$BIN pane wait-output $KID --match KID-42 --timeout 3000"
assert "with command-from-panes off it reads only its own pane" '[ "$RC" != 0 ] && grep -aq "only read its own pane" "$O"'
inpane 0 "$BIN pane wait-output 0 --match pane_holder --timeout 3000"
assert "and that still works" '[ "$RC" = 0 ]'
$BIN set -g command-from-panes spawned >/dev/null 2>&1

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
