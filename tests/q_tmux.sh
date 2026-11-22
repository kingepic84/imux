#!/bin/bash
# `tmux-compat`: a pane gets $TMUX, $TMUX_PANE and a `tmux` first on PATH that runs imux's commands
# with tmux's command line; the calls fzf-tab and `fzf --tmux` make (display-message with the
# pane's geometry and option names as variables, list-commands, display-popup with attached flag
# values and the exit status of its program); and `tmux-compat off`.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qt.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/bash
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

$BIN new-session -d -s tm >/dev/null 2>&1
sleep 1

say() {
  $BIN send-keys -t "${2:-%0}" C-u >/dev/null 2>&1
  $BIN send-keys -t "${2:-%0}" -l "$1" >/dev/null 2>&1
  $BIN send-keys -t "${2:-%0}" Enter >/dev/null 2>&1
}
shows() { $BIN capture-pane -p -t "${2:-%0}" | grep -aq -- "$1"; }
wait_for() {
  local n=$1
  shift
  while [ "$n" -gt 0 ]; do
    if "$@"; then return 0; fi
    sleep 0.3
    n=$((n - 1))
  done
  return 1
}

say 'printf "BEFORE:%s\n" "$(command -v tmux)"'
sleep 1
say "eval \"\$($BIN init bash)\""
sleep 1
say 'printf "ENV:%s:%s\n" "$TMUX_PANE" "$(command -v tmux)"'
wait_for 20 shows 'ENV:%0:'
assert "a pane has TMUX_PANE" 'shows "ENV:%0:"'
assert "and a tmux that is the shim" 'shows "/shims/tmux"'
say 'case "$TMUX" in */default,[0-9]*,0) echo TMUXOK;; *) echo "TMUXBAD:$TMUX";; esac'
wait_for 20 shows 'TMUXOK'
assert "TMUX is socket, pid and session" 'shows TMUXOK'

say 'tmux -V | sed "s/^/VER:/"'
wait_for 20 shows 'VER:tmux 3'
assert "tmux -V prints a tmux version line" 'shows "VER:tmux 3\.[0-9]*$"'

say 'tmux display-message -p "#{pane_top} #{cursor_y} #{pane_left} #{cursor_x} #{window_height} #{window_width} #{status} #{status-position}" | sed "s/^/FT:/"'
wait_for 20 shows 'FT:'
assert "display-message gives the geometry, the cursor and option names as variables" 'shows "FT:0 [0-9]* 0 [0-9]* [0-9]* [0-9]* on bottom$"'

say 'tmux display-message -p -t "$TMUX_PANE" "#{window_width} #{window_height}" \; list-commands new-pane 2>/dev/null; echo RC:$?'
wait_for 20 shows 'RC:'
assert "the check fzf makes sees no new-pane command" 'shows "RC:1" && ! shows "^new-pane"'

say 'tmux list-commands display-popup'
wait_for 20 shows '^display-popup$'
assert "list-commands names a command it has" 'shows "^display-popup$"'

say 'tmux no-such-thing 2>&1 | sed "s/^/ERR:/"'
wait_for 20 shows 'ERR:'
assert "an unknown command is an error on stderr" 'shows "ERR:"'

client() {
  (sleep 2; "$@"; sleep 1; printf '\002d'; sleep 0.5) | timeout 60 script -q "$TMPDIR/c.out" sh -c "stty rows 30 cols 100; $BIN attach -t tm" >/dev/null 2>&1
}

k_status() {
  say 'for c in 0 1 130; do tmux popup -E sh -c "exit $c"; echo PS$c:$?; done'
  sleep 6
}
client k_status
assert "display-popup ends with the status of its program (0)" 'shows "PS0:0"'
assert "display-popup ends with the status of its program (1)" 'shows "PS1:1"'
assert "display-popup ends with the status of its program (130)" 'shows "PS130:130"'

printf 'stty size > %s/sz\necho FTB > %s/res\n' "$TMPDIR" "$TMPDIR" >"$TMPDIR/ftb"
k_ftb() {
  say "tmux popup -x 5 -y 12 -w 30 -h 8 -d \$PWD -E \". $TMPDIR/ftb\"; echo FTBDONE:\$?"
  sleep 4
}
client k_ftb
assert "a popup with attached or separate flag values runs a sourced file and returns" 'shows "FTBDONE:0" && grep -aq FTB "$TMPDIR/res"'
assert "its screen is the box inside the border" '[ "$(cat "$TMPDIR/sz")" = "6 28" ]'

k_fzf() {
  say "tmux display-popup -E -d \$PWD -xC -yC -w80% -h50% sh $TMPDIR/ftb; echo FZFDONE:\$?"
  sleep 4
}
client k_fzf
assert "the shape of the fzf call works: attached values and a program with arguments" 'shows "FZFDONE:0"'

if command -v fzf >/dev/null 2>&1; then
  k_realfzf() {
    say "printf 'alpha\nbeta\n' | fzf --tmux -q alp --select-1 > $TMPDIR/fzfout; echo REALFZF:\$?"
    sleep 5
  }
  client k_realfzf
  assert "fzf --tmux opens a popup through the shim and returns its choice" 'shows "REALFZF:0" && [ "$(cat "$TMPDIR/fzfout" 2>/dev/null)" = alpha ] && grep -aq "opened by pane" "$TMPDIR/imux-$(id -u)/server.log"'
else
  echo "skip fzf --tmux (fzf is not installed)"
fi

FTBLIB=/opt/homebrew/opt/fzf-tab/share/fzf-tab/lib
if command -v fzf >/dev/null 2>&1 && command -v zsh >/dev/null 2>&1 && [ -f "$FTBLIB/ftb-tmux-popup" ]; then
  printf 'alpha\nbeta\ngamma\n' >"$TMPDIR/ftbin"
  printf 'zmodload zsh/mapfile\nfpath=(%s $fpath)\nautoload -Uz ftb-tmux-popup\n_ftb_curcontext=x\nftb-tmux-popup -q alp --select-1 < %s/ftbin > %s/ftbout\necho RET:$? > %s/ftbret\n' "$FTBLIB" "$TMPDIR" "$TMPDIR" "$TMPDIR" >"$TMPDIR/ftb.zsh"
  k_ftbtab() {
    say "zsh -f $TMPDIR/ftb.zsh; echo FTBTAB"
    sleep 6
  }
  client k_ftbtab
  assert "fzf-tab's own ftb-tmux-popup places and runs its popup and returns the choice" '[ "$(cat "$TMPDIR/ftbout" 2>/dev/null)" = alpha ] && [ "$(cat "$TMPDIR/ftbret" 2>/dev/null)" = RET:0 ]'
else
  echo "skip fzf-tab (fzf, zsh or fzf-tab is not installed)"
fi

$BIN set -g tmux-compat off >/dev/null 2>&1
$BIN new-window -d -n off >/dev/null 2>&1
sleep 1
OFFP=$($BIN list-panes -t tm:off -F '#{pane_id}' 2>/dev/null | head -1)
say 'echo "OFF:${TMUX_PANE:-none}:${TMUX:-none}"' "$OFFP"
wait_for 20 shows 'OFF:' "$OFFP"
assert "with tmux-compat off a new pane has none of it" 'shows "OFF:none:none" "$OFFP"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
