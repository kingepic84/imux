#!/bin/bash
# P10.10, the agent skill: `imux skill` prints the file built into the binary, `imux skill path`
# says where an agent looks for it, `imux doctor` reports whether the installed copy is there and
# the same, and imux writes nothing outside its own state directory.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qsk.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
SRC="$(cd "$(dirname "$0")/.." && pwd)/skills/imux/SKILL.md"
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

run() {
  OUT="$TMPDIR/out"
  ERR="$TMPDIR/err"
  timeout 30 $BIN "$@" >"$OUT" 2>"$ERR" </dev/null
  RC=$?
}

export HOME="$TMPDIR/home"
mkdir -p "$HOME"

run skill
assert "imux skill prints the shipped file, byte for byte" '[ $RC = 0 ] && cmp -s "$OUT" "$SRC"'
run skill path
assert "skill path says where an agent looks" '[ $RC = 0 ] && [ "$(cat $OUT)" = "$HOME/.claude/skills/imux/SKILL.md" ]'
run skill bogus
assert "a word the tree does not know is a usage error" '[ $RC = 2 ]'
run skill --help
assert "skill has its help" '[ $RC = 0 ] && grep -aq "path" "$OUT"'

run doctor
assert "doctor says the skill is not installed" 'grep -aq "^skill: not installed" "$OUT"'
assert "and that nothing was written for it" '[ ! -e "$HOME/.claude" ]'
mkdir -p "$HOME/.claude/skills/imux"
printf 'old\n' >"$HOME/.claude/skills/imux/SKILL.md"
run doctor
assert "doctor says an installed copy differs" 'grep -aq "^skill: installed at .* but different" "$OUT"'
$BIN skill >"$HOME/.claude/skills/imux/SKILL.md" 2>/dev/null
run doctor
assert "and when it is the same" 'grep -aq "^skill: installed at .*, the same as this imux" "$OUT"'

run completions zsh
assert "completions know skill" 'grep -aq "skill" "$OUT"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
