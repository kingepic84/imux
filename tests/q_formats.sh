#!/bin/bash
# Format modifiers (=N: b: d: n: l: m: m/r: E: T: e|op|:), the variables added with them (pid,
# socket_path, version, pane_tty, window_last_flag, pane_at_*, one-letter forms, environment names),
# the same in the status bar, and the %if / %elif / %else / %endif / %hidden lines of a config.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-qf.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 120; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

mkdir -p "$TMPDIR/proj/sub"
printf '%%hidden GREETING=hello\nset -g @greet "${GREETING}"\n%%if "#{>=:#{version},3.0}"\nset -g @ver new\n%%elif "#{==:1,1}"\nset -g @ver mid\n%%else\nset -g @ver old\n%%endif\n%%if "#{==:#{version},0.1}"\nset -g @skipped yes\n%%endif\n%%if #{TEST_COND_ENV}\nset -g @env yes\n%%else\nset -g @env no\n%%endif\n%%if "#{m:*x*,axb}"\n%%if "#{e|>|:2,1}"\nset -g @nest ok\n%%endif\n%%endif\nset -g status-left "S:#{b:pane_current_path}:#{version}:#{pane_at_left}:"\nset -g status-right ""\nset -g @tpl "[#{session_name}]"\n' >"$IMUX_CONFIG"

TEST_COND_ENV=1 $BIN new-session -d -s s -n one -c "$TMPDIR/proj" >/dev/null 2>&1
$BIN new-window -t s -n two -c "$TMPDIR/proj/sub" >/dev/null 2>&1
$BIN new-window -t s -n three >/dev/null 2>&1
$BIN select-window -t s:two >/dev/null 2>&1
$BIN select-window -t s:one >/dev/null 2>&1

fm() {
  $BIN display-message -p -t s:one "$1" 2>&1
}

echo "-- modifiers"
assert "=N: keeps the first characters" '[ "$(fm "#{=3:session_name}")" = "s" ] && [ "$(fm "#{=3:window_name}")" = "one" ]'
assert "=-N: keeps the last ones" '[ "$(fm "#{=-2:window_name}")" = "ne" ]'
assert "b: is the last path component" '[ "$(fm "#{b:pane_current_path}")" = "proj" ]'
assert "d: is the rest" '[ "$(fm "#{b:#{d:pane_current_path}}")" = "$(basename "$TMPDIR")" ]'
assert "n: counts characters" '[ "$(fm "#{n:window_name}")" = 3 ]'
assert "l: is a literal" '[ "$(fm "#{l:window_name}")" = "window_name" ]'
glob_yes=$(fm '#{m:o*,#{window_name}}')
glob_no=$(fm '#{m:x*,#{window_name}}')
assert "m: matches a glob" '[ "$glob_yes" = 1 ] && [ "$glob_no" = 0 ]'
re_yes=$(fm '#{m/r:^o.e$,#{window_name}}')
assert "m/r: matches a regular expression" '[ "$re_yes" = 1 ]'
re_ci=$(fm '#{m/ri:ONE,#{window_name}}')
assert "m/ri: ignores case" '[ "$re_ci" = 1 ]'
sum=$(fm '#{e|+|:#{window_index},10}')
assert "e|op|: does arithmetic" '[ "$sum" = 10 ] || [ "$sum" = 11 ]'
cond_match=$(fm '#{?#{m:*one*,#{window_name}},yes,no}')
assert "a conditional can test a match" '[ "$cond_match" = yes ]'
assert "E: expands a user option as a format" '[ "$(fm "#{E:@tpl}")" = "[s]" ] && [ "$(fm "#{@tpl}")" = "[#{session_name}]" ]'
assert "T: expands the clock too" '[ "$(fm "#{T:@tpl}")" = "[s]" ]'

echo "-- variables"
assert "pid is a number" '[[ "$(fm "#{pid}")" =~ ^[0-9]+$ ]]'
assert "socket_path ends in the socket name" '[[ "$(fm "#{socket_path}")" == */default ]]'
assert "version is the one the shim reports" '[ "$(fm "#{version}")" = 3.4 ]'
assert "pane_tty is a device" '[[ "$(fm "#{pane_tty}")" == /dev/* ]]'
assert "#D is the pane id" '[ "$(fm "#D")" = "$(fm "#{pane_id}")" ] && [[ "$(fm "#D")" == %* ]]'
assert "#h and #H are the host" '[ -n "$(fm "#h")" ] && [ "$(fm "#H")" = "$(fm "#{host}")" ] && [ "$(fm "#h")" = "$(fm "#{host_short}")" ]'
assert "#F and #T are the flags and the title" '[ "$(fm "#F")" = "$(fm "#{window_flags}")" ] && [ "$(fm "#T")" = "$(fm "#{pane_title}")" ]'
assert "pane_at_* tell the edges of a lone pane" '[ "$(fm "#{pane_at_left}#{pane_at_top}#{pane_at_right}#{pane_at_bottom}")" = 0000 ] || [ "$(fm "#{pane_at_left}#{pane_at_top}#{pane_at_right}#{pane_at_bottom}")" = 1111 ]'
assert "pane_mode is empty outside copy mode" '[ -z "$(fm "#{pane_mode}")" ]'
assert "session_many_attached is a flag" '[ "$(fm "#{session_many_attached}")" = 0 ]'
$BIN split-window -h -t s:one >/dev/null 2>&1
assert "a split pane has an inner edge" '[ "$($BIN display-message -p -t s:one.0 "#{pane_at_left}#{pane_at_right}")" = 10 ] || [ "$($BIN display-message -p -t s:one.1 "#{pane_at_left}#{pane_at_right}")" = 01 ]'
$BIN kill-pane -t s:one.1 >/dev/null 2>&1
$BIN select-window -t s:one >/dev/null 2>&1
lastflags=$($BIN list-windows -t s -F "#{window_name}=#{window_last_flag}" | tr "\n" " ")
assert "window_last_flag marks the window that was current before" '[ "$lastflags" = "one=0 two=1 three=0 " ]'

echo "-- environment names"
$BIN set-environment -g IMUX_FMT_TEST hello-env >/dev/null 2>&1
assert "a name that is no variable reads the environment set for panes" '[ "$(fm "#{IMUX_FMT_TEST}")" = hello-env ]'
assert "an unset one is empty" '[ -z "$(fm "#{IMUX_FMT_NOT_SET_ANYWHERE}")" ]'

echo "-- the configuration with %if lines"
assert "%hidden makes a variable of the file" '[ "$(fm "#{@greet}")" = hello ]'
assert "a true %if runs its branch and not the %elif" '[ "$(fm "#{@ver}")" = new ]'
assert "a false %if is skipped" '[ -z "$(fm "#{@skipped}")" ]'
assert "a condition can read the environment" '[ "$(fm "#{@env}")" = yes ]'
assert "conditions nest" '[ "$(fm "#{@nest}")" = ok ]'

echo "-- the status bar"
(sleep 3; printf '\002d'; sleep 0.5) \
  | timeout 30 script -q "$TMPDIR/c1.out" sh -c "stty rows 24 cols 100; $BIN attach -t s" >/dev/null 2>&1 &
CL=$!
wait $CL
assert "status-left expands a modifier, a new variable and a pane variable" 'grep -aqE "S:proj:3\.4:[01]:" "$TMPDIR/c1.out"'

echo "-- a config that is wrong"
$BIN kill-server >/dev/null 2>&1
sleep 1
printf '%%if yes\nset -g @a 1\n' >"$IMUX_CONFIG"
$BIN check-config "$IMUX_CONFIG" >"$TMPDIR/cc.out" 2>&1
assert "check-config reads the conditional lines without calling them commands" '! grep -aq "unknown-command: %if" "$TMPDIR/cc.out"'
printf '%%endif\n' >"$IMUX_CONFIG"
$BIN new-session -d -s e -n x >/dev/null 2>&1
sleep 1
$BIN show-options >/dev/null 2>&1
assert "a server with a stray %endif still starts" '$BIN ls >/dev/null 2>&1'
$BIN kill-server >/dev/null 2>&1

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
