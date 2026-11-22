#!/bin/bash
# `imux check-config`: what a configuration holds that imux does not take, by file and line, with
# counts; a sourced file is read; the exit status; --summary; the default file; and that it starts no
# server, writes no socket directory and changes no file.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qcc.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG=/dev/null
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

printf 'set -g nosuchoption 1\n' >"$TMPDIR/other.conf"
printf 'set -g status-left "#{N:session_name} #{nope}"\nbind x frobnicate\nsplit-window -Q\nbind -T copy-mode-vi y send -X frobnicate\nset -g @plugin y\nsource-file %s/other.conf\nsource-file %s/gone.conf\nset -g mouse on\n' "$TMPDIR" "$TMPDIR" >"$TMPDIR/main.conf"
printf 'set -g mouse on\nbind r source-file ~/.imux.conf\n' >"$TMPDIR/clean.conf"
before=$(cksum "$TMPDIR/main.conf" "$TMPDIR/other.conf")

$BIN check-config "$TMPDIR/main.conf" >"$TMPDIR/out" 2>&1
rc=$?
assert "a configuration with problems exits 1" '[ "$rc" = 1 ]'
assert "every problem is named with its file and line" 'grep -aq "main.conf:2: unknown-command: frobnicate" "$TMPDIR/out" && grep -aq "main.conf:3: unknown-flag: split-window -Q" "$TMPDIR/out" && grep -aq "main.conf:1: unknown-variable: nope" "$TMPDIR/out" && grep -aq "main.conf:1: unknown-modifier: N:" "$TMPDIR/out"'
assert "a sourced file is read, with its own file and line" 'grep -aq "other.conf:1: unknown-option: nosuchoption" "$TMPDIR/out"'
assert "what is understood and does nothing is listed as information" 'grep -aq "ignored-command: bind" "$TMPDIR/out" && grep -aq "user-option: @plugin" "$TMPDIR/out" && grep -aq "missing-file: .*gone.conf" "$TMPDIR/out"'
assert "the counts by kind and the totals follow" 'grep -aq "^files: 2, lines: 9, problems: " "$TMPDIR/out" && grep -aq "^unknown-flag: 1$" "$TMPDIR/out"'

$BIN check-config --summary "$TMPDIR/main.conf" >"$TMPDIR/sum" 2>&1
assert "--summary leaves out the line per problem" '! grep -aq "main.conf:2:" "$TMPDIR/sum" && grep -aq "^unknown-command: 1$" "$TMPDIR/sum"'

$BIN check-config "$TMPDIR/clean.conf" >"$TMPDIR/clean.out" 2>&1
assert "a clean file exits 0" '[ $? = 0 ] && grep -aq "problems: 0" "$TMPDIR/clean.out"'

IMUX_CONFIG="$TMPDIR/clean.conf" $BIN check-config >"$TMPDIR/def.out" 2>&1
assert "without a file it reads the configuration imux would read" 'grep -aq "files: 1" "$TMPDIR/def.out"'

$BIN check-config "$TMPDIR/missing.conf" >"$TMPDIR/miss.out" 2>&1
assert "a file that cannot be read is a problem" '[ $? = 1 ] && grep -aq "unreadable" "$TMPDIR/miss.out"'

assert "no file was changed" '[ "$(cksum "$TMPDIR/main.conf" "$TMPDIR/other.conf")" = "$before" ]'
assert "no server was started and no socket directory made" '! ls "$TMPDIR"/imux-* >/dev/null 2>&1 && ! $BIN ls >/dev/null 2>&1'

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
