#!/bin/bash
# strip-environment: per-window identity and agent-session variables of the launch terminal are not
# handed to new panes; capability hints and other settings still are; set-environment and an emptied
# option put things back.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qse.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

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

(sleep 100; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

launch() {
  printf '%s\n' "$1" >"$IMUX_CONFIG"
  $BIN kill-server >/dev/null 2>&1
  sleep 0.5
  rm -f "$TMPDIR/env.out"
  ITERM_SESSION_ID=w0t0p0 KITTY_WINDOW_ID=7 ZELLIJ_SESSION_NAME=z STY=123.pts WINDOW=3 \
    CLAUDECODE=1 CLAUDE_CODE_SESSION_ID=abc CLAUDE_CODE_USE_BEDROCK=1 CLAUDE_CONFIG_DIR=/tmp/cfg \
    TERM_PROGRAM=iTerm.app LC_TERMINAL=iTerm2 MYVAR_ONE=1 \
    $BIN new-session -d -s se "env >$TMPDIR/env.out; sleep 20" >/dev/null 2>&1
  until_true '[ -s "$TMPDIR/env.out" ]' 40
}

has() {
  grep -aq "^$1=" "$TMPDIR/env.out"
}

launch 'set -g @none 1'
assert "ITERM_SESSION_ID is not handed on" '! has ITERM_SESSION_ID'
assert "KITTY_WINDOW_ID is not handed on" '! has KITTY_WINDOW_ID'
assert "ZELLIJ* is not handed on" '! has ZELLIJ_SESSION_NAME'
assert "STY is not handed on" '! has STY'
assert "WINDOW goes with STY" '! has WINDOW'
assert "CLAUDECODE is not handed on" '! has CLAUDECODE'
assert "CLAUDE_CODE_SESSION_ID is not handed on" '! has CLAUDE_CODE_SESSION_ID'
assert "TERM_PROGRAM stays" 'has TERM_PROGRAM'
assert "LC_TERMINAL stays" 'has LC_TERMINAL'
assert "CLAUDE_CODE_USE_BEDROCK stays" 'has CLAUDE_CODE_USE_BEDROCK'
assert "CLAUDE_CONFIG_DIR stays" 'has CLAUDE_CONFIG_DIR'
assert "an unlisted variable stays" 'has MYVAR_ONE'
assert "IMUX_ENV is set" 'has IMUX_ENV'

launch 'set -g strip-environment ""'
assert "an empty option strips nothing" 'has ITERM_SESSION_ID && has CLAUDECODE && has STY'

launch 'set -g strip-environment "MYVAR_*"'
assert "a start with a star is stripped" '! has MYVAR_ONE'
assert "and the defaults are replaced, not added to" 'has ITERM_SESSION_ID'

launch 'set -g @none 1'
$BIN set-environment ITERM_SESSION_ID mine >/dev/null 2>&1
rm -f "$TMPDIR/env.out"
$BIN new-window -d -t se: "env >$TMPDIR/env.out; sleep 20" >/dev/null 2>&1
until_true '[ -s "$TMPDIR/env.out" ]' 40
assert "set-environment puts a stripped name back" 'grep -aq "^ITERM_SESSION_ID=mine$" "$TMPDIR/env.out"'
assert "the others are still stripped" '! has KITTY_WINDOW_ID'

launch 'set-environment -g ITERM_SESSION_ID fromconf'
assert "set-environment in the configuration reaches the first pane" 'grep -aq "^ITERM_SESSION_ID=fromconf$" "$TMPDIR/env.out"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
