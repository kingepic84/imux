#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-fds.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

printf 'set autosave-interval 0\n' >"$IMUX_CONFIG"

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aqF -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) &

cat >"$TMPDIR/fds.sh" <<'EOF'
#!/bin/bash
lsof -a -p $PPID -d 3-999 -Fftn >"$1" 2>&1
perl -e 'print defined $SIG{PIPE} ? $SIG{PIPE} : "default"' >"$2" 2>&1
{ ulimit -n; ulimit -c; } >"$3" 2>&1
EOF
chmod +x "$TMPDIR/fds.sh"
{ ulimit -n; ulimit -c; } >"$TMPDIR/limits-here.txt"

echo "-- a pane shell inherits nothing of the server"
ARGS="new -s w" HOLD=3 client "$TMPDIR/a.out" \
  '\002c' "$TMPDIR/fds.sh $TMPDIR/fds.txt $TMPDIR/pipe.txt $TMPDIR/limits-pane.txt\r" >/dev/null
check "the listing ran" "f" "$TMPDIR/fds.txt"
refute "no pipe of the server (the signal pipe)" "tPIPE" "$TMPDIR/fds.txt"
refute "no listening or client socket" "tunix" "$TMPDIR/fds.txt"
refute "no event queue" "tKQUEUE" "$TMPDIR/fds.txt"
refute "no pty master of another pane" "n/dev/ptmx" "$TMPDIR/fds.txt"
refute "SIGPIPE is not left ignored" "IGNORE" "$TMPDIR/pipe.txt"
if cmp -s "$TMPDIR/limits-here.txt" "$TMPDIR/limits-pane.txt"; then
  echo "ok   the pane gets the open file and core limits the server was started with"
else
  echo "FAIL the pane limits differ: here $(tr '\n' ' ' <"$TMPDIR/limits-here.txt") pane $(tr '\n' ' ' <"$TMPDIR/limits-pane.txt")"
  fail=1
fi
SOCKMODE=$(stat -f %Lp "$TMPDIR/imux-$(id -u)/default" 2>/dev/null)
if [ "$SOCKMODE" = 600 ]; then echo "ok   the socket is private"; else echo "FAIL socket mode $SOCKMODE"; fail=1; fi

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
