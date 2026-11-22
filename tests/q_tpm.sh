#!/bin/bash
# `run` and the `tmux` shim: a plugin script started from the configuration (as tpm starts its
# plugins) drives imux through `tmux set`, `bind-key`, `set-environment` and reads back through
# `display-message` and `show-environment`.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
HERE="$(cd "$(dirname "$0")" && pwd)"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qp.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
export PLUG_OUT="$TMPDIR"
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

until_true() {
  n=0
  while [ "$n" -lt "$2" ]; do
    if eval "$1"; then return 0; fi
    sleep 0.25
    n=$((n + 1))
  done
  return 1
}

(sleep 90; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cp "$HERE/fixtures/plugin.tmux" "$TMPDIR/plugin.tmux"
chmod +x "$TMPDIR/plugin.tmux"
printf '%s\n' \
  "set -g @plugin 'someone/plugin'" \
  "run '$TMPDIR/plugin.tmux'" \
  >"$IMUX_CONFIG"

$BIN new-session -d -s tp -n start >/dev/null 2>&1

assert "the plugin script ran and set an option through the shim" 'until_true "[ \"\$($BIN show-options -gv @plug-ran)\" = yes ]" 40'
assert "tmux -V inside the script is the shim's answer" 'until_true "$BIN show-options -gv @plug-version | grep -aq \"tmux 3\"" 20'
assert "a binding made by the script exists" 'until_true "$BIN list-keys | grep -aq \"rename-window .plugged\"" 20'
assert "a root binding with a command and flags exists" 'until_true "$BIN list-keys | grep -aq \"new-window -n from-plugin\"" 20'
assert "the script read back its own option" 'until_true "[ \"\$(cat $TMPDIR/echo.out 2>/dev/null)\" = yes ]" 20'
assert "set-environment and show-environment round trip" 'until_true "grep -aq PLUG_ENV=one $TMPDIR/env.out 2>/dev/null" 20'
assert "the script has TMUX set" '[ "$(cat $TMPDIR/tmuxvar.out 2>/dev/null)" = tmux-set ]'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
