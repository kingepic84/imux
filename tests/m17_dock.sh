#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-dock.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/conf"
fail=0
LOG="$TMPDIR/imux-$(id -u)/server.log"

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 0
set agent-panel dock
set agent-dock-position left
set agent-dock-width 30
EOF

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.9; done; sleep "${HOLD:-1}"; printf '\002d'; sleep 1) \
    | timeout 90 script -q "$out" sh -c "stty rows 30 cols 100; $BIN $ARGS"
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) &

cat >"$TMPDIR/agent.sh" <<'EOF'
#!/bin/bash
printf '\033]7501;state=working:app=fake:msg=%s\033\\' "$(printf '%s' 'dock-msg' | base64)"
sleep 40
EOF
chmod +x "$TMPDIR/agent.sh"

cat >"$TMPDIR/reader.sh" <<'EOF'
#!/bin/bash
printf '\033]7501;state=blocked:app=fake:kind=permission\033\\'
IFS= read -r line </dev/tty
printf '%s' "$line" >"$1"
sleep 30
EOF
chmod +x "$TMPDIR/reader.sh"

echo "-- the dock takes room from the panes and gives it back"
ARGS="new -s d" HOLD=2 client "$TMPDIR/a.out" \
  'echo before-$(tput cols)\r' \
  '\002c' "$TMPDIR/agent.sh\r" \
  '\002' '0' \
  '\002A' \
  'wait' \
  '\033' \
  'echo mid-$(tput cols)\r' \
  '\002A' 'A' \
  'echo after-$(tput cols)\r' >/dev/null
check "panes start at the full width" "before-100" "$TMPDIR/a.out"
check "the dock lists the agent with its message" "dock-msg" "$TMPDIR/a.out"
check "the dock has a title" "agents" "$TMPDIR/a.out"
check "with the dock open the panes are narrower" "mid-70" "$TMPDIR/a.out"
check "hiding the dock gives the width back" "after-100" "$TMPDIR/a.out"

$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- Enter in the dock jumps to the agent and the dock stays"
ARGS="new -s e" HOLD=3 client "$TMPDIR/b.out" \
  '\002c' "$TMPDIR/reader.sh $TMPDIR/got.txt\r" \
  '\002c' \
  '\002A' \
  '\r' "hello-from-the-dock\r" \
  'echo' >/dev/null
sleep 1
assert "Enter jumps to the first row and keys go to that pane" '[ "$(cat "$TMPDIR/got.txt" 2>/dev/null)" = "hello-from-the-dock" ]'

$BIN kill-server >/dev/null 2>&1
sleep 1
grep -a '"level":"error"' "$LOG" && { echo "FAIL errors in server.log"; fail=1; }
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
