#!/bin/bash
# Plugins (M39): link and its refusals, a plugin that follows events, permissions (a denied
# request names the permission, a granted one works, a plugin cannot kill the server or follow an
# event it did not list), a clean environment, restart with backoff and giving up, `restart never`,
# a clean exit, enable and disable at run time, and stopping with the server (SIGTERM, then SIGKILL
# for one that ignores it). Needs a debug build: IMUX_TEST_PLUGIN_BACKOFF_MS shortens the delays.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-plugins.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_PLUGINS_DIR="$TMPDIR/linked"
export IMUX_CONFIG=/dev/null
export IMUX_TEST_PLUGIN_BACKOFF_MS=20
export SECRET_TEST=leaked
export SHELL=/bin/sh
fail=0
T="$TMPDIR"
LOG="$T/imux-$(id -u)/server.log"

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

(sleep 150; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

# mk name "manifest lines" "script body"
mk() {
  local dir="$T/src/$1"
  mkdir -p "$dir"
  printf '%s\n' "$2" >"$dir/plugin.conf"
  printf '#!/bin/sh\n%s\n' "$3" >"$dir/run.sh"
  chmod 755 "$dir/run.sh"
  chmod 644 "$dir/plugin.conf"
}

echo "-- link"
mk bad-api "name bad-api
api 2
exec ./run.sh" "exit 0"
mk bad-perm "name bad-perm
api 1
exec ./run.sh
permissions shell" "exit 0"
mk bad-event "name bad-event
api 1
exec ./run.sh
events no-such-event" "exit 0"
mk bad-exec "name bad-exec
api 1
exec ../outside.sh" "exit 0"
for p in bad-api bad-perm bad-event bad-exec; do
  $BIN plugin link "$T/src/$p" >"$T/$p.out" 2>&1
done
assert "a manifest for another api is refused" 'grep -aq "needs api 2" $T/bad-api.out'
assert "an unknown permission is refused and the known ones listed" 'grep -aq "unknown permission" $T/bad-perm.out && grep -aq "read-panes" $T/bad-perm.out'
assert "an unknown event is refused" 'grep -aq "unknown event type" $T/bad-event.out'
assert "an exec outside the directory is refused" 'grep -aq "inside the plugin directory" $T/bad-exec.out'
assert "nothing was linked" '[ -z "$(ls $T/linked 2>/dev/null)" ]'
chmod 777 "$T/src/bad-api/run.sh" 2>/dev/null
mk writable "name writable
api 1
exec ./run.sh" "exit 0"
chmod 666 "$T/src/writable/run.sh"
$BIN plugin link "$T/src/writable" >"$T/writable.out" 2>&1
assert "a program others can write is refused" 'grep -aq "can be written by others" $T/writable.out'

mk watch "name watch
api 1
exec ./run.sh
events window-linked" "\"\$IMUX_EXE\" events --types window-linked | while read -r line; do echo \"\$line\" >> $T/watch.out; done"
mk nosend "name nosend
api 1
exec ./run.sh
events window-linked" "echo \"plugin=\$IMUX_PLUGIN secret=\${SECRET_TEST:-none} socket=\${IMUX_SOCKET_PATH:+set}\"
until \"\$IMUX_EXE\" list-sessions 2>/dev/null | grep -q '^a: '; do sleep 0.2; done
\"\$IMUX_EXE\" list-sessions
\"\$IMUX_EXE\" send-keys -t a: nope
\"\$IMUX_EXE\" new-window -d -t a -n sneaky
\"\$IMUX_EXE\" kill-server
\"\$IMUX_EXE\" events --types pane-died
\"\$IMUX_EXE\" run-shell 'touch $T/ran-shell'
\"\$IMUX_EXE\" plugin-reload
echo finished
sleep 3000"
mk sender "name sender
api 1
exec ./run.sh
permissions send-keys windows" "sleep 1
\"\$IMUX_EXE\" send-keys -t a:0 'echo from-plugin > $T/sender.out' Enter
\"\$IMUX_EXE\" new-window -d -t a -n by-plugin
sleep 3000"
mk crash "name crash
api 1
exec ./run.sh" "echo x >> $T/crash.count
echo boom >&2
exit 1"
mk never "name never
api 1
exec ./run.sh
restart never" "echo x >> $T/never.count
exit 1"
mk clean "name clean
api 1
exec ./run.sh" "echo x >> $T/clean.count
exit 0"
mk sleeper "name sleeper
api 1
exec ./run.sh" "echo x >> $T/sleeper.count
exec sleep 3333"
mk stubborn "name stubborn
api 1
exec ./run.sh" "trap '' TERM
echo started >> $T/stubborn.count
sleep 4444 &
wait"

for p in watch nosend sender crash never clean stubborn; do
  $BIN plugin link "$T/src/$p" >/dev/null 2>&1
  $BIN plugin enable "$p" >/dev/null 2>&1
done
$BIN plugin link "$T/src/sleeper" >/dev/null 2>&1
assert "a linked plugin is listed with its permissions" '$BIN plugin list | grep -aq "^sender.*enabled.*permissions: send-keys windows" &&$BIN plugin list | grep -aq "^sleeper.*disabled"'
assert "linking again is harmless" '$BIN plugin link "$T/src/sleeper" | grep -aq "linked already"'

echo "-- start with the server"
$BIN new-session -d -s a -n first >/dev/null 2>&1
assert "an enabled plugin runs" 'until_true "pgrep -f \"events --types window-linked\" >/dev/null" 40'
assert "a plugin that is not enabled does not" '[ ! -e $T/sleeper.count ]'
$BIN new-window -d -t a -n second >/dev/null 2>&1
assert "a plugin follows the events it listed" 'until_true "grep -aq window-linked $T/watch.out" 40'

echo "-- permissions"
assert "a plugin that has run to its end" 'until_true "$BIN plugin logs nosend | grep -aq finished" 60'
$BIN plugin logs nosend >"$T/nosend.log"
assert "it has the plugin name, the socket and no secret from the server" 'grep -aq "plugin=nosend secret=none socket=set" $T/nosend.log'
assert "what only looks is allowed" 'grep -aq "^a: " $T/nosend.log'
assert "typing is denied and names the permission" 'grep -aq "plugin nosend may not send-keys: it has no permission send-keys" $T/nosend.log'
assert "windows are denied and name the permission" 'grep -aq "plugin nosend may not new-window: it has no permission windows" $T/nosend.log && ! $BIN list-windows -t a -F "#{window_name}" | grep -aq sneaky'
assert "killing the server is denied" 'grep -aq "plugin nosend may not do that" $T/nosend.log && $BIN has-session -t a'
assert "an event it did not list is denied" 'grep -aq "plugin nosend may not follow pane-died" $T/nosend.log'
assert "a shell command is denied" 'grep -aq "plugin nosend may not run run-shell" $T/nosend.log && [ ! -e $T/ran-shell ]'
assert "restarting plugins is denied" 'grep -aq "plugin nosend may not run plugin-reload" $T/nosend.log'
assert "a plugin with the permissions can use them" 'until_true "[ -s $T/sender.out ]" 60 && $BIN list-windows -t a -F "#{window_name}" | grep -aq by-plugin'
assert "the user can still do all of it" '$BIN new-window -d -t a -n user-made && $BIN list-windows -t a -F "#{window_name}" | grep -aq user-made'

echo "-- restarts"
assert "a plugin that fails is started again, five times, and then given up" 'until_true "[ \"\$(wc -l < $T/crash.count | tr -d \" \")\" = 6 ]" 60'
sleep 2
assert "it is not started a seventh time" '[ "$(wc -l < $T/crash.count | tr -d " ")" = 6 ]'
assert "plugin list says it stopped and why" '$BIN plugin list | grep -aq "^crash.*stopped: ended 6 times"'
assert "what it printed is in its log" '$BIN plugin logs crash | grep -aq boom'
assert "restart never means once" '[ "$(wc -l < $T/never.count | tr -d " ")" = 1 ]'
assert "a plugin that ends cleanly is not started again" '[ "$(wc -l < $T/clean.count | tr -d " ")" = 1 ]'
assert "the log of a stopped plugin says why in the server log" 'grep -aq "plugin crash stopped for good" $LOG'

echo "-- enable and disable at run time"
$BIN plugin enable sleeper >"$T/en.out" 2>&1
assert "enable starts it" 'grep -aq "started sleeper" $T/en.out && until_true "pgrep -f \"sleep 3333\" >/dev/null" 40'
$BIN plugin disable sleeper >"$T/dis.out" 2>&1
assert "disable stops it with its children" 'until_true "! pgrep -f \"sleep 3333\" >/dev/null" 40 && grep -aq "stopping sleeper" $T/dis.out'
assert "and it stays off" '$BIN plugin list | grep -aq "^sleeper.*disabled"'
$BIN plugin enable sleeper >/dev/null 2>&1
assert "enable again starts it again" 'until_true "[ \"\$(wc -l < $T/sleeper.count | tr -d \" \")\" = 2 ]" 40'
$BIN plugin unlink sleeper >/dev/null 2>&1
assert "unlink stops it and removes the link" 'until_true "! pgrep -f \"sleep 3333\" >/dev/null" 40 && [ ! -e $T/linked/sleeper ] && [ -d $T/src/sleeper ]'

echo "-- the server ends"
assert "a plugin that ignores SIGTERM is running" 'until_true "pgrep -f \"sleep 4444\" >/dev/null" 40'
assert "and so is the one that follows events" 'pgrep -f "events --types window-linked" >/dev/null'
$BIN kill-server >/dev/null 2>&1
assert "the plugins end with the server, the stubborn one by SIGKILL" 'until_true "! pgrep -f \"sleep 4444\" >/dev/null && ! pgrep -f \"events --types window-linked\" >/dev/null" 40'
assert "no errors in the server log but the ones about the crashing plugin" '! grep -a "\"level\":\"error\"" $LOG | grep -avq "plugin crash"'

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
