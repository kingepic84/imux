#!/bin/bash
# Command line tools that need no server: --version (and, as they are built, doctor and completions).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-q.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0

check() {
  if grep -aqE -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

echo "-- --version"
for word in --version -V version; do
  $BIN $word >"$TMPDIR/v" 2>&1
  code=$?
  assert "$word exits 0" "[ $code -eq 0 ]"
  check "$word prints the name and a version" '^imux [0-9]+\.[0-9]+\.[0-9]+' "$TMPDIR/v"
  assert "$word prints one line" '[ "$(wc -l <"$TMPDIR/v" | tr -d " ")" = 1 ]'
done
assert "no server was started" '[ ! -e "$TMPDIR/imux-$(id -u)" ]'
$BIN --version extra >"$TMPDIR/v2" 2>&1
assert "extra words after --version are refused" '[ $? -ne 0 ]'
check "and the usage names --version" '--version' "$TMPDIR/v2"

echo "-- doctor"
printf 'set -g mouse on\nset -g image-probe-timeout soon\nbogus-command x\nset -g default-terminal tmux-256color\n' >"$TMPDIR/bad.conf"
IMUX_CONFIG="$TMPDIR/bad.conf" $BIN doctor >"$TMPDIR/d1" 2>&1
assert "doctor exits 0 without a terminal" '[ $? -eq 0 ]'
check "it says there is no terminal to ask" '^terminal probe: not asked: no terminal$' "$TMPDIR/d1"
check "a bad value is named with file and line" 'bad\.conf:2: ' "$TMPDIR/d1"
check "an unknown command is named with file and line" 'bad\.conf:3: ' "$TMPDIR/d1"
check "the problems are counted" '^config check: 2 problem\(s\)$' "$TMPDIR/d1"
check "an option without effect is listed" '^config option without effect: default-terminal$' "$TMPDIR/d1"
check "the glyphs are shown" '^agent glyphs: working=' "$TMPDIR/d1"
assert "doctor started no server" '[ ! -e "$TMPDIR/imux-$(id -u)" ]'
assert "doctor wrote no state" '[ ! -e "$IMUX_STATE_DIR" ]'

printf 'set -g image-probe never\n' >"$TMPDIR/never.conf"
CLEAN="env -u LC_TERMINAL -u COLORTERM -u TERM_PROGRAM -u KITTY_WINDOW_ID TERM=xterm-256color"
IMUX_CONFIG="$TMPDIR/never.conf" timeout 30 script -q "$TMPDIR/d2" sh -c "stty rows 30 cols 100; $CLEAN $BIN doctor" >/dev/null 2>&1
check "image-probe never is explained" 'terminal probe: not asked: image-probe is never' "$TMPDIR/d2"
check "no pixel size is explained under a terminal" 'cell size: unknown: the terminal reports no pixel size \(100x30 cells\)' "$TMPDIR/d2"
check "so is the hint about cropped pictures" 'hint: without a pixel size pictures are cropped' "$TMPDIR/d2"
check "an unprobed unknown terminal gets blocks" 'picture mode: blocks ' "$TMPDIR/d2"
check "and is told how to ask" 'image-probe always' "$TMPDIR/d2"

printf 'set -g image-probe always\nset -g image-probe-timeout 300\n' >"$TMPDIR/always.conf"
IMUX_CONFIG="$TMPDIR/always.conf" timeout 30 script -q "$TMPDIR/d3" sh -c "stty rows 30 cols 100; $CLEAN $BIN doctor" >/dev/null 2>&1
check "a terminal that never answers is reported" 'terminal probe: the terminal did not finish answering' "$TMPDIR/d3"
check "and nothing is ruled out then" 'picture protocols accepted: everything \(the terminal did not answer' "$TMPDIR/d3"

$BIN new-session -d -s a >/dev/null 2>&1
IMUX_CONFIG=/dev/null $BIN doctor >"$TMPDIR/d4" 2>&1
check "a running server is reported with its pid" '^server: running \(pid [0-9]+\)$' "$TMPDIR/d4"
check "and the socket directory is checked" 'private, ours' "$TMPDIR/d4"

$BIN kill-server >/dev/null 2>&1
sleep 1

echo "-- completions"
$BIN completions zsh >"$TMPDIR/c.zsh" 2>&1
assert "completions zsh exits 0" '[ $? -eq 0 ]'
$BIN completions bash >"$TMPDIR/c.bash" 2>&1
$BIN completions fish >"$TMPDIR/c.fish" 2>&1
$BIN completions pwsh >"$TMPDIR/c.pwsh" 2>&1
$BIN completions nu >"$TMPDIR/c.nu" 2>&1
check "pwsh registers a completer" '^Register-ArgumentCompleter -Native -CommandName imux' "$TMPDIR/c.pwsh"
check "nu defines the extern" '^extern "imux" ' "$TMPDIR/c.nu"
check "zsh starts with its header" '^#compdef imux$' "$TMPDIR/c.zsh"
check "bash hooks the function up" '^complete -F _imux imux$' "$TMPDIR/c.bash"
check "fish switches file completion off" '^complete -c imux -f$' "$TMPDIR/c.fish"
for shell in zsh bash fish pwsh nu; do
  check "$shell offers commands" 'new-window' "$TMPDIR/c.$shell"
  check "$shell offers client words" 'kill-server' "$TMPDIR/c.$shell"
  check "$shell offers an option" 'status-position' "$TMPDIR/c.$shell"
  check "$shell offers a hook" 'alert-bell' "$TMPDIR/c.$shell"
  check "$shell offers the shells of completions" 'zsh' "$TMPDIR/c.$shell"
done
for shell in zsh bash fish; do
  if command -v $shell >/dev/null 2>&1; then
    $shell -n "$TMPDIR/c.$shell" >"$TMPDIR/n.$shell" 2>&1
    assert "$shell accepts its script" '[ $? -eq 0 ]'
  else
    echo "skip no $shell: its syntax check"
  fi
done
if command -v pwsh >/dev/null 2>&1; then
  pwsh -NoProfile -Command "\$e = \$null; [System.Management.Automation.Language.Parser]::ParseFile('$TMPDIR/c.pwsh', [ref]\$null, [ref]\$e) | Out-Null; if (\$e.Count -gt 0) { \$e | ForEach-Object { \$_.Message }; exit 1 }" >"$TMPDIR/n.pwsh" 2>&1
  assert "pwsh parses its script" '[ $? -eq 0 ]'
else
  echo "skip no pwsh: its syntax check"
fi
if command -v nu >/dev/null 2>&1; then
  nu -c "source '$TMPDIR/c.nu'; if false { imux set -g status; imux new -s x }; nu-complete imux top | length" >"$TMPDIR/n.nu" 2>&1
  assert "nu sources its script and the externs accept flags" '[ $? -eq 0 ] && grep -aqE "^[0-9]+$" "$TMPDIR/n.nu"'
  nu -c "source '$TMPDIR/c.nu'; nu-complete imux set-option | where \$it == status-position | length" >"$TMPDIR/n2.nu" 2>&1
  check "nu offers an option for set" '^1$' "$TMPDIR/n2.nu"
else
  echo "skip no nu: its syntax check"
fi
$BIN completions csh >"$TMPDIR/c.csh" 2>&1
assert "an unknown shell is refused" '[ $? -ne 0 ]'
check "with the shells named" 'zsh, bash, fish, pwsh, nu' "$TMPDIR/c.csh"
$BIN completions ksh >"$TMPDIR/c.ksh" 2>&1
assert "ksh is refused" '[ $? -ne 0 ]'
check "with the reason" 'no programmable completion' "$TMPDIR/c.ksh"
$BIN completions >"$TMPDIR/c.none" 2>&1
assert "no shell word is refused" '[ $? -ne 0 ]'
assert "completions started no server" '[ ! -e "$TMPDIR/imux-$(id -u)/default" ]'
if command -v bash >/dev/null 2>&1; then
  bash -c "source '$TMPDIR/c.bash'; COMP_WORDS=(imux new-w); COMP_CWORD=1; _imux; printf '%s\n' \"\${COMPREPLY[@]}\"" >"$TMPDIR/b1" 2>&1
  check "bash completes a command word" '^new-window$' "$TMPDIR/b1"
  bash -c "source '$TMPDIR/c.bash'; COMP_WORDS=(imux set -g status-p); COMP_CWORD=3; _imux; printf '%s\n' \"\${COMPREPLY[@]}\"" >"$TMPDIR/b2" 2>&1
  check "bash completes an option after set" '^status-position$' "$TMPDIR/b2"
  bash -c "source '$TMPDIR/c.bash'; COMP_WORDS=(imux set-hook alert-b); COMP_CWORD=2; _imux; printf '%s\n' \"\${COMPREPLY[@]}\"" >"$TMPDIR/b3" 2>&1
  check "bash completes a hook after set-hook" '^alert-bell$' "$TMPDIR/b3"
  bash -c "source '$TMPDIR/c.bash'; COMP_WORDS=(imux agent l); COMP_CWORD=2; _imux; printf '%s\n' \"\${COMPREPLY[@]}\"" >"$TMPDIR/b4" 2>&1
  check "bash completes a sub-command" '^list$' "$TMPDIR/b4"
fi

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
