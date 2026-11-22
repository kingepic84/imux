#!/bin/bash
# monitor-silence and silence-action: a window nobody views that printed nothing for the given
# seconds gets the ~ flag (and #{window_silence_flag}); printing again takes it off; 0 turns the
# monitor off.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qq.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

flags() {
  $BIN list-windows -t q -F "#{window_name}=#{window_flags}" 2>&1
}

until_flags() {
  local pattern=$1 i
  for i in $(seq 1 40); do
    flags | grep -aqE -- "$pattern" && return 0
    sleep 0.25
  done
  return 1
}

$BIN new-session -d -s q -n cur >/dev/null 2>&1
$BIN new-window -d -t q -n quiet >/dev/null 2>&1
sleep 1

assert "off by default: no flag" '! flags | grep -aq "~"'
EVS="$TMPDIR/silence.log"
$BIN set-hook -g alert-silence "run-shell \"echo silence:#{q:hook_window_name} >> $EVS\"" >/dev/null 2>&1
$BIN set -g monitor-silence 2 >/dev/null 2>&1
assert "monitor-silence is a real option" '$BIN show-options -gv monitor-silence | grep -aqx 2'
assert "a quiet window gets the flag" 'until_flags "quiet=.*~"'
assert "the variable says so" '[ "$($BIN display-message -p -t q:quiet "#{window_silence_flag}")" = 1 ]'
sleep 1
assert "the alert-silence hook ran for that window" 'grep -aqx "silence:quiet" "$EVS"'
assert "the current window is left alone with the default silence-action" '! flags | grep -aq "cur=.*~"'

$BIN send-keys -t q:quiet 'echo noise' Enter >/dev/null 2>&1
assert "printing takes the flag off" 'until_flags "quiet=[^~]*$"'
assert "and it comes back after the quiet time" 'until_flags "quiet=.*~"'

$BIN set -g silence-action none >/dev/null 2>&1
$BIN send-keys -t q:quiet 'echo again' Enter >/dev/null 2>&1
sleep 3.5
assert "silence-action none sets no flag" '! flags | grep -aq "quiet=.*~"'

$BIN set -g silence-action other >/dev/null 2>&1
$BIN set -g monitor-silence 0 >/dev/null 2>&1
sleep 1.5
assert "monitor-silence 0 takes the flag away" '! flags | grep -aq "~"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
