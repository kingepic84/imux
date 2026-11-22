#!/bin/bash
# `{ }` command blocks in configuration files, `source-file` and the `:` prompt's command lines: a
# binding with several commands, blocks over several lines with comments, nested blocks in
# `if-shell`, a hook, and errors that name the line.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qb.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

cat >"$IMUX_CONFIG" <<'EOF'
bind F5 { new-window; rename-window one }
bind F6 {
  # three commands, one per line
  new-window
  rename-window "two words"   # a trailing comment }
  select-window -t 0
}
bind F7 { if-shell "true" { rename-window yes } { rename-window no } }
set-hook -g session-renamed { display-message "renamed" ; select-window -t 0 }
EOF

$BIN new-session -d -s blk >/dev/null 2>&1
$BIN list-keys >"$TMPDIR/keys" 2>&1
assert "the config with blocks loaded" '[ -s "$TMPDIR/keys" ]'
assert "a one line block binds a chain" 'grep -aE "F5 +new-window \\\\; rename-window \"?one\"?$" "$TMPDIR/keys" >/dev/null'
assert "a block over several lines binds a chain" 'grep -aE "F6 +new-window \\\\; rename-window \"?two words\"? \\\\; select-window" "$TMPDIR/keys" >/dev/null'
assert "the comment inside it is gone" '! grep -aq "trailing comment" "$TMPDIR/keys"'
assert "a nested block binds an if-shell" 'grep -aq "F7" "$TMPDIR/keys"'
$BIN show-hooks >"$TMPDIR/hooks" 2>&1
assert "a hook takes a block" 'grep -aq "session-renamed" "$TMPDIR/hooks" && grep -aq "display-message" "$TMPDIR/hooks"'

printf '%s\n' 'if-shell "true" {' '  set -g status-interval 7' '  set -g display-time 1234' '} {' '  set -g status-interval 9' '}' >"$TMPDIR/ok.conf"
$BIN source-file "$TMPDIR/ok.conf" >"$TMPDIR/s1" 2>&1
sleep 1
$BIN show-options >"$TMPDIR/o1" 2>&1
assert "if-shell runs its first block when the test passes" 'grep -aq "^status-interval 7$" "$TMPDIR/o1"'
assert "all the commands of the block" 'grep -aq "^display-time 1234$" "$TMPDIR/o1"'

printf '%s\n' 'if-shell "false" {' '  set -g status-interval 11' '} {' '  set -g status-interval 13' '  set -g display-time 4321' '}' >"$TMPDIR/ok2.conf"
$BIN source-file "$TMPDIR/ok2.conf" >"$TMPDIR/s2" 2>&1
sleep 1
$BIN show-options >"$TMPDIR/o2" 2>&1
assert "and its second block when it fails" 'grep -aq "^status-interval 13$" "$TMPDIR/o2" && grep -aq "^display-time 4321$" "$TMPDIR/o2"'

printf '%s\n' 'set -g status on' 'display-message ok' '}' >"$TMPDIR/stray.conf"
$BIN source-file "$TMPDIR/stray.conf" >"$TMPDIR/s3" 2>&1
assert "a stray } is an error that names the line" 'grep -aq "stray.conf:3" "$TMPDIR/s3" && grep -aq "unexpected }" "$TMPDIR/s3"'

printf '%s\n' 'set -g status on' 'bind F8 {' '  new-window' 'set -g status off' >"$TMPDIR/open.conf"
$BIN source-file "$TMPDIR/open.conf" >"$TMPDIR/s4" 2>&1
assert "a block that never closes is an error that names the line it began on" 'grep -aq "open.conf:2" "$TMPDIR/s4" && grep -aq "no matching }" "$TMPDIR/s4"'

printf '%s\n' 'bind F9 { new-window; frobnicate }' >"$TMPDIR/bad.conf"
$BIN source-file "$TMPDIR/bad.conf" >"$TMPDIR/s5" 2>&1
assert "a bad command inside a block is reported" 'grep -aq "bad.conf:1" "$TMPDIR/s5" && grep -aq "unknown command" "$TMPDIR/s5"'

$BIN list-keys >"$TMPDIR/keys2" 2>&1
assert "the keys of the bad files are not bound" '! grep -aq "F8\|F9" "$TMPDIR/keys2"'

printf '%s\n' 'bind F10 { display-message "a } b" ; display-message '"'"'c ; d'"'"' }' >"$TMPDIR/quote.conf"
$BIN source-file "$TMPDIR/quote.conf" >"$TMPDIR/s6" 2>&1
$BIN list-keys >"$TMPDIR/keys3" 2>&1
assert "braces and semicolons inside quotes stay in the quotes" 'grep -aq "F10" "$TMPDIR/keys3" && grep -aq "a } b" "$TMPDIR/keys3"'

(sleep 2; printf '\002:'; sleep 0.3; printf 'rename-window { x }'; sleep 0.3; printf '\r'; sleep 1; printf '\002d'; sleep 0.5) | timeout 20 script -q "$TMPDIR/c1.out" sh -c "stty rows 30 cols 100; $BIN attach -t blk" >/dev/null 2>&1
$BIN list-windows -F '#{window_name}' >"$TMPDIR/w3" 2>&1
assert "the prompt takes a block too" 'grep -aqx x "$TMPDIR/w3"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
