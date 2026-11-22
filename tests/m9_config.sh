#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-config.XXXXXX)
fail=0

cat > "$TMPDIR/test.conf" <<'EOF'
# test configuration
set -g base-index 1
set -g prefix C-a
bind v split-window -h
unbind %
set -g bogus-option 1
bind r source-file /nonexistent/imux.conf \; display "reload tried"
EOF
export IMUX_CONFIG="$TMPDIR/test.conf" IMUX_STATE_DIR="$TMPDIR/state"

check() {
  if grep -aq "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

run_client() {
  local out=$1; shift
  (sleep 1; for chunk in "$@"; do printf '%b' "$chunk"; sleep 0.7; done; sleep 1) \
    | timeout 30 script -q "$out" sh -c "stty rows 30 cols 100; $BIN"
}

(sleep 11; $BIN kill-server >/dev/null 2>&1) &
run_client "$TMPDIR/a.out" \
  '\001v' \
  'echo bound-size-$(stty size | tr " " x)\r' \
  '\001%' \
  'echo unbound-size-$(stty size | tr " " x)\r' \
  '\001:' \
  'set -g base-index 7\r' \
  '\001c' \
  '\001r' \
  '\001:' \
  'set -g nonsense 1\r'

check "config: bind v splits side by side" "bound-size-29x49" "$TMPDIR/a.out"
check "config: unbound % does nothing" "unbound-size-29x49" "$TMPDIR/a.out"
check "config: base-index 1 numbers the first tab 1" "1:[a-z]" "$TMPDIR/a.out"
check "prompt: set base-index 7 renumbers tabs" "8:[a-z]" "$TMPDIR/a.out"
check "config: bind with chained display" "source-file\|reload tried\|error(s)" "$TMPDIR/a.out"
check "prompt: unknown option shows a message" "unknown option: nonsense" "$TMPDIR/a.out"

sleep 1
check "config: bad option logged with line number" "test.conf:6: unknown option: bogus-option" "$TMPDIR/imux-$(id -u)/server.log"
grep -a '"level":"error"' "$TMPDIR/imux-$(id -u)/server.log" && { echo "FAIL errors in server.log"; fail=1; }
rm -rf "$TMPDIR"
exit $fail
