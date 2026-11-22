#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-clip.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"
ESC=$'\033'
BEL=$'\007'

has() {
  if grep -aqF -- "${ESC}]52;$2${BEL}" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

lacks() {
  if grep -aqF -- "${ESC}]52;$2${BEL}" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.8; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) &

cat >"$TMPDIR/clip.sh" <<'EOF'
#!/bin/bash
for step in "$@"; do
  case $step in
    wait:*) sleep "${step#wait:}" ;;
    query) printf '\033]52;c;?\007' ;;
    *) printf '\033]52;%s;%s\007' "${step%%:*}" "${step#*:}" ;;
  esac
done
sleep 5
EOF
chmod +x "$TMPDIR/clip.sh"
C="$TMPDIR/clip.sh"

echo "-- by default a pane cannot set the clipboard"
printf 'set autosave-interval 0\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=2 client "$TMPDIR/a.out" "$C c:Zmlyc3Q=\r" >/dev/null
lacks "set-clipboard external ignores the pane's request" "c;Zmlyc3Q=" "$TMPDIR/a.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- off ignores it too"
printf 'set autosave-interval 0\nset set-clipboard off\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=2 client "$TMPDIR/b.out" "$C c:Zmlyc3Q=\r" >/dev/null
lacks "set-clipboard off ignores the pane's request" "c;Zmlyc3Q=" "$TMPDIR/b.out"
$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- on passes well formed writes, at most one a second, and nothing else"
printf 'set autosave-interval 0\nset set-clipboard on\n' >"$IMUX_CONFIG"
ARGS="new -s w" HOLD=3 client "$TMPDIR/c.out" \
  "$C c:Zmlyc3Q= c:c2Vjb25k query c:a_b x:aGk= wait:1.3 :dGhpcmQ=\r" >/dev/null
has "a valid write is passed on" "c;Zmlyc3Q=" "$TMPDIR/c.out"
lacks "a second write within a second is dropped" "c;c2Vjb25k" "$TMPDIR/c.out"
lacks "the read form is dropped" "c;?" "$TMPDIR/c.out"
lacks "a payload that is not base64 is dropped" "c;a_b" "$TMPDIR/c.out"
lacks "a target that is not a selection letter is dropped" "x;aGk=" "$TMPDIR/c.out"
has "an empty target means the clipboard, and a later write passes" "c;dGhpcmQ=" "$TMPDIR/c.out"

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
