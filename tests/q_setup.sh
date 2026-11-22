#!/bin/bash
# imux setup puts the manual and the agent skill where they are read, and leaves them alone when they
# are already right. HOME is a private directory, so nothing outside the test is touched.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qs.XXXXXX)
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
SKILL="$($BIN skill path 2>/dev/null)"

echo "-- first run"
$BIN setup >"$TMPDIR/o1" 2>&1
assert "setup succeeds" '[ $? = 0 ]'
assert "the manual is installed" '[ -s "$MAN" ]'
assert "the manual is a man page" 'head -c 200 "$MAN" | grep -aq "^\.TH"'
assert "the skill is installed where skill path says" '[ -s "$SKILL" ] && [ "$SKILL" = "$HOME/.claude/skills/imux/SKILL.md" ]'
assert "the skill is what the binary prints" '[ "$($BIN skill)" = "$(cat "$SKILL")" ]'
assert "it says what it did" 'grep -aq "manual: installed" "$TMPDIR/o1" && grep -aq "skill: installed" "$TMPDIR/o1"'
assert "doctor sees the skill as installed" '$BIN doctor 2>/dev/null | grep -aq "skill.*the same as this imux"'

echo "-- second run"
$BIN setup >"$TMPDIR/o2" 2>&1
assert "nothing is rewritten" 'grep -aq "manual: already installed" "$TMPDIR/o2" && grep -aq "skill: already installed" "$TMPDIR/o2"'

echo "-- an older copy is brought up to date"
printf 'old\n' >"$SKILL"
$BIN setup >"$TMPDIR/o3" 2>&1
assert "the skill is updated" 'grep -aq "skill: updated" "$TMPDIR/o3" && [ "$($BIN skill)" = "$(cat "$SKILL")" ]'

echo "-- choosing"
rm -f "$MAN" "$SKILL"
$BIN setup --no-man >"$TMPDIR/o4" 2>&1
assert "--no-man leaves the manual out" '[ ! -e "$MAN" ] && [ -s "$SKILL" ]'
rm -f "$SKILL"
$BIN setup --no-skill --man-dir "$TMPDIR/elsewhere" >"$TMPDIR/o5" 2>&1
assert "--no-skill and --man-dir" '[ -s "$TMPDIR/elsewhere/man1/imux.1" ] && [ ! -e "$SKILL" ]'
$BIN setup --no-man --skill-path "$TMPDIR/my/SKILL.md" >"$TMPDIR/o6" 2>&1
assert "--skill-path" '[ -s "$TMPDIR/my/SKILL.md" ]'

echo "-- the shell startup file"
rm -rf "$HOME/.zshrc" "$HOME/.config"
$BIN setup --no-man --no-skill >"$TMPDIR/s1" 2>&1
assert "the zsh line is added to .zshrc" '[ "$(grep -c "imux init zsh" "$HOME/.zshrc")" = 1 ] && grep -qx "eval \"\$(imux init zsh)\"" "$HOME/.zshrc"'
assert "and the report shows the line" 'grep -aq "shell: added to" "$TMPDIR/s1"'
$BIN setup --no-man --no-skill >"$TMPDIR/s2" 2>&1
assert "a second run adds nothing" '[ "$(grep -c "imux init" "$HOME/.zshrc")" = 1 ] && grep -aq "shell: already set up" "$TMPDIR/s2"'
$BIN setup --no-man --no-skill --shell fish >"$TMPDIR/s3" 2>&1
assert "--shell fish uses config.fish and fish syntax" 'grep -qx "imux init fish | source" "$HOME/.config/fish/config.fish"'
SHELL=/bin/bash $BIN setup --no-man --no-skill >"$TMPDIR/s4" 2>&1
assert "bash goes to .bashrc" 'grep -qx "eval \"\$(imux init bash)\"" "$HOME/.bashrc"'
printf 'export KEEP=1\n' >"$HOME/.keep"
rm -f "$HOME/.zshrc"; cp "$HOME/.keep" "$HOME/.zshrc"
$BIN setup --no-man --no-skill >"$TMPDIR/s5" 2>&1
assert "an existing .zshrc keeps its text, first" '[ "$(head -1 "$HOME/.zshrc")" = "export KEEP=1" ] && grep -q "imux init zsh" "$HOME/.zshrc"'
rm -f "$HOME/.zshrc"
$BIN setup --no-man --no-skill --no-shell >"$TMPDIR/s6" 2>&1
assert "--no-shell touches no startup file" '[ ! -e "$HOME/.zshrc" ]'
SHELL=/usr/bin/nu $BIN setup --no-man --no-skill >"$TMPDIR/s7" 2>&1
assert "a shell imux init does not know is said, not guessed" 'grep -aq "not for /usr/bin/nu" "$TMPDIR/s7" && [ ! -e "$HOME/.zshrc" ]'
ZDOTDIR="$TMPDIR/zd" $BIN setup --no-man --no-skill >"$TMPDIR/s8" 2>&1
assert "ZDOTDIR moves the zsh file" 'grep -q "imux init zsh" "$TMPDIR/zd/.zshrc"'
rm -f "$HOME/.zshrc"

echo "-- a symlink is not written through"
mkdir -p "$TMPDIR/dots"
printf 'mine\n' >"$TMPDIR/dots/skill"
rm -f "$SKILL"
ln -s "$TMPDIR/dots/skill" "$SKILL"
$BIN setup --no-man >"$TMPDIR/o7" 2>&1
assert "the link is left alone and says so" 'grep -aq "symlink" "$TMPDIR/o7" && [ "$(cat "$TMPDIR/dots/skill")" = mine ]'

echo "-- it needs a home"
env -u HOME "$BIN" setup >"$TMPDIR/o8" 2>&1
assert "no HOME is an error that says so" '[ $? != 0 ] || grep -aq "HOME" "$TMPDIR/o8"'
env -u HOME "$BIN" setup >"$TMPDIR/o8" 2>&1; rc=$?
assert "and exits non-zero" '[ $rc != 0 ]'

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
