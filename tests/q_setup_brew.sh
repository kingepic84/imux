#!/bin/bash
# imux setup says nothing about the manpage, and writes nothing, when the manpage came with the binary
# (Homebrew puts it in the keg next to bin/imux and links the binary). An explicit --man-dir still
# installs there, and without such a page it installs as always. HOME is private.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qsb.XXXXXX)
export HOME="$TMPDIR/home"
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
unset XDG_DATA_HOME ZDOTDIR XDG_CONFIG_HOME
export SHELL=/bin/zsh
mkdir -p "$HOME"
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

MAN="$HOME/.local/share/man/man1/imux.1"
KEG="$TMPDIR/Cellar/imux/0.1.0"
mkdir -p "$KEG/bin" "$TMPDIR/prefix/bin"
cp "$BIN" "$KEG/bin/imux"
ln -s "$KEG/bin/imux" "$TMPDIR/prefix/bin/imux"

echo "-- no page next to the binary: installed as always"
"$TMPDIR/prefix/bin/imux" setup --no-skill --no-shell >"$TMPDIR/o0" 2>&1
assert "the manpage is installed" '[ -s "$MAN" ]'
assert "it says so" 'grep -aq "manual: installed" "$TMPDIR/o0"'
rm -rf "$HOME/.local"

echo "-- a page in the keg, run through the symlink"
mkdir -p "$KEG/share/man/man1"
printf '.TH IMUX 1\n' >"$KEG/share/man/man1/imux.1"
"$TMPDIR/prefix/bin/imux" setup --no-skill --no-shell >"$TMPDIR/o1" 2>&1
assert "setup succeeds" '[ $? = 0 ]'
assert "it prints nothing" '[ ! -s "$TMPDIR/o1" ]'
assert "nothing is written under HOME" '[ ! -e "$MAN" ]'

echo "-- the other steps still run"
"$TMPDIR/prefix/bin/imux" setup --no-shell --skill-path "$TMPDIR/skill/SKILL.md" >"$TMPDIR/o2" 2>&1
assert "the skill is installed" '[ -s "$TMPDIR/skill/SKILL.md" ] && grep -aq "skill: installed" "$TMPDIR/o2"'
assert "and the manpage is still not mentioned" '! grep -aqi "manual" "$TMPDIR/o2"'

echo "-- an explicit --man-dir installs there anyway"
"$TMPDIR/prefix/bin/imux" setup --no-skill --no-shell --man-dir "$TMPDIR/chosen" >"$TMPDIR/o3" 2>&1
assert "the page is written where it was asked" '[ -s "$TMPDIR/chosen/man1/imux.1" ] && grep -aq "manual: installed" "$TMPDIR/o3"'

echo "-- our own copy under HOME is still kept up to date"
mkdir -p "$(dirname "$MAN")"
printf 'old\n' >"$MAN"
"$TMPDIR/prefix/bin/imux" setup --no-skill --no-shell >"$TMPDIR/o4" 2>&1
assert "it is updated" 'grep -aq "manual: updated" "$TMPDIR/o4" && head -c 200 "$MAN" | grep -aq "^\.TH"'

rm -rf "$TMPDIR"
exit $fail
