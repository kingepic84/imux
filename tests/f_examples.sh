#!/bin/bash
# The example plugins under examples/plugins (M40), linked and run for real: notify-blocked (a fake
# osascript records the notification, hostile text stays data), worktree-names (a window that holds
# a worktree gets the worktree's name back after it was renamed), git-branch (sets the status
# segment once; branch.sh prints a branch or nothing). Needs a debug build.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
SRC="$(cd "$(dirname "$0")/.." && pwd)/examples/plugins"
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-examples.XXXXXX)
T="$TMPDIR"
export T
export IMUX_STATE_DIR="$T/state"
export IMUX_PLUGINS_DIR="$T/linked"
export IMUX_CONFIG="$T/conf"
export SHELL=/bin/sh
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
fail=0
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

cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 0
set worktree-base main
set worktree-name-pattern "[A-Z]+-[0-9]+"
EOF

mkdir -p "$T/bin"
cat >"$T/bin/osascript" <<EOF
#!/bin/sh
for a in "\$@"; do printf '%s\n' "\$a"; done >>"$T/osascript.log"
printf -- '--end--\n' >>"$T/osascript.log"
EOF
chmod 755 "$T/bin/osascript"
export PATH="$T/bin:$PATH"

echo "-- link"
for p in notify-blocked worktree-names git-branch; do
  $BIN plugin link "$SRC/$p" >"$T/link-$p.out" 2>&1
  assert "$p links" 'grep -aq "linked" $T/link-$p.out'
  $BIN plugin enable "$p" >/dev/null 2>&1
done
assert "the permissions are the ones the manifests ask for" '$BIN plugin list | grep -aq "^worktree-names.*permissions: windows" && $BIN plugin list | grep -aq "^git-branch.*permissions: set-options"'

echo "-- repository"
W="$T/work"
mkdir -p "$W"
git init -q --bare -b main "$T/origin.git"
git clone -q "$T/origin.git" "$W/proj" 2>/dev/null
(
  cd "$W/proj" || exit 1
  git checkout -q -b main
  echo one >a.txt
  git add a.txt
  git commit -q -m one
  git push -q origin main
)

$BIN new-session -d -s a -n first >/dev/null 2>&1
assert "the server and the plugins are up" 'until_true "pgrep -f \"events --types agent-state-changed\" >/dev/null && pgrep -f \"events --types window-linked,window-renamed\" >/dev/null" 40'

echo "-- git-branch"
assert "it set the status segment" 'until_true "$BIN show-options -g status-right | grep -aq \"branch.sh\"" 40'
assert "with the directory quoted by imux" '$BIN show-options -g status-right | grep -aq "#{q:pane_current_path}"'
assert "branch.sh prints the branch" '[ "$("$SRC/git-branch/branch.sh" "$W/proj")" = main ]'
assert "and nothing outside a repository" '[ -z "$("$SRC/git-branch/branch.sh" "$T")" ] && [ -z "$("$SRC/git-branch/branch.sh" "$T/missing")" ]'
assert "plugin list shows it ended cleanly, not stopped" '! $BIN plugin list | grep -aq "^git-branch.*stopped"'

echo "-- worktree-names"
(cd "$W/proj" && $BIN worktree ABC-1 >"$T/wt.out" 2>&1)
assert "a worktree window exists" 'grep -aq "ABC-1" $T/wt.out && until_true "$BIN list-windows -t a -F \"#{window_name}\" | grep -aqx ABC-1" 40'
WID=$($BIN list-windows -t a -F '#{window_id} #{window_name}' | awk '$2 == "ABC-1" { print $1 }')
$BIN rename-window -t "$WID" something-else >/dev/null 2>&1
assert "renamed away, the plugin gives the worktree's name back" 'until_true "$BIN list-windows -t a -F \"#{window_name}\" | grep -aqx ABC-1" 40'
$BIN rename-window -t a:first plain >/dev/null 2>&1
sleep 1
assert "a window without a worktree is left alone" '$BIN list-windows -t a -F "#{window_name}" | grep -aqx plain'

echo "-- notify-blocked"
cat >"$T/agent.sh" <<'EOF'
#!/bin/bash
n=0
r() { n=$((n + 1)); "$BIN" pane report-agent --source fake --agent fake --state "$1" --seq "$n" ${2:+--message "$2"} >/dev/null 2>&1; }
r idle
: >"$T/agent.ready"
while IFS= read -r line </dev/tty; do
  case "$line" in
    *block*) r blocked "approve Bash: \"; do shell script \"touch $T/pwned\" -- $line" ;;
    *) r working; sleep 1; r done ;;
  esac
done
EOF
chmod +x "$T/agent.sh"
AP=$($BIN new-window -d -P -F '#{pane_id}' -t a -n agent | tr -d '%')
sleep 1
$BIN send-keys -t "%$AP" "$T/agent.sh" Enter >/dev/null 2>&1
assert "the fake agent is up" 'until_true "[ -f $T/agent.ready ]" 40'
$BIN send-keys -t "%$AP" "work" Enter >/dev/null 2>&1
sleep 2
assert "no notification for working or done" '[ ! -e $T/osascript.log ]'
$BIN send-keys -t "%$AP" "block" Enter >/dev/null 2>&1
assert "a blocked agent gives a notification" 'until_true "grep -aq -- --end-- $T/osascript.log" 40'
assert "with the window name in the title argument" 'grep -aqx "agent" $T/osascript.log'
assert "and the agent's message as one argument, text and not script" 'grep -aq "approve Bash" $T/osascript.log && [ ! -e $T/pwned ]'

$BIN kill-server >/dev/null 2>&1
assert "the plugins end with the server" 'until_true "! pgrep -f \"events --types agent-state-changed\" >/dev/null" 40'
assert "no errors in the server log" '! grep -a "\"level\":\"error\"" $LOG'

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
