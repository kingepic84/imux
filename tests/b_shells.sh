#!/bin/bash
# What imux types into a pane shell must work in every shell it knows: the worktree setup chained
# to the command (and a failing setup stopping it), and a restored program coming back with its
# hostile arguments intact. Runs for each of bash, zsh, fish, nu and pwsh that is installed.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-shells.XXXXXX)
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.com GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.com
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_SYSTEM=/dev/null
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

until_true() {
  local n=0
  while [ "$n" -lt "$2" ]; do
    eval "$1" && return 0
    sleep 0.5
    n=$((n + 1))
  done
  return 1
}

lines_of() {
  if [ -f "$1" ]; then wc -l <"$1" | tr -d ' '; else echo 0; fi
}

W="$TMPDIR/work"
mkdir -p "$W"
git init -q --bare -b main "$TMPDIR/origin.git"
git clone -q "$TMPDIR/origin.git" "$W/proj" 2>/dev/null
(
  cd "$W/proj" || exit 1
  git checkout -q -b main
  echo one >a.txt
  git add a.txt
  git commit -q -m one
  git push -q origin main
)

HOSTILE="it's \$HOME;\`id\` 'q' \"d\" -n"

one_shell() {
  local name=$1 path=$2 n=$3
  local run touch false_
  case "$name" in
    nu) run="^/bin/sleep"; touch="^/usr/bin/touch"; false_="^/usr/bin/false" ;;
    pwsh) run="& /bin/sleep"; touch="& /usr/bin/touch"; false_="& /usr/bin/false" ;;
    *) run="/bin/sleep"; touch="/usr/bin/touch"; false_="/usr/bin/false" ;;
  esac
  local d="$TMPDIR/$name"
  mkdir -p "$d"
  export IMUX_STATE_DIR="$d/state"
  export IMUX_CONFIG="$d/conf"
  export TMPDIR_SAVE="$TMPDIR"
  local sock_dir="$d/sock"
  mkdir -p "$sock_dir"
  local old_tmp="$TMPDIR"
  export TMPDIR="$sock_dir"
  cat >"$IMUX_CONFIG" <<EOF
set autosave-interval 0
set default-shell $path
set worktree-base main
set worktree-name-pattern "[A-Z]+-[0-9]+"
set worktree-command "$run $((31400 + n))"
set restore-commands "perl"
EOF
  echo "== $name ($path)"

  (sleep 1; sleep 90; printf '\002d'; sleep 1) | timeout 120 script -q "$d/c.out" sh -c "stty rows 30 cols 100; $BIN new -s w" >/dev/null 2>&1 &
  local client=$!
  until_true "$BIN ls 2>/dev/null | grep -q '^w:'" 40

  (cd "$W/proj" && $BIN worktree --setup "$touch $d/ok.marker" "ABC-$n" >"$d/o1" 2>&1)
  assert "$name: the setup ran" "until_true '[ -e \"$d/ok.marker\" ]' 60"
  assert "$name: and the command started after it" "until_true 'pgrep -f \"sleep $((31400 + n))\" >/dev/null' 30"
  pkill -f "sleep $((31400 + n))" 2>/dev/null

  (cd "$W/proj" && $BIN worktree --setup "$false_" "ABD-$n" >"$d/o2" 2>&1)
  sleep 12
  assert "$name: a failing setup keeps the command from starting" '! pgrep -f "sleep $((31400 + n))" >/dev/null'
  pkill -f "sleep $((31400 + n))" 2>/dev/null

  echo "-- $name: a restored program keeps its arguments"
  printf '%s\n' "open(F, \">>$d/args.txt\"); print F \$ARGV[0], \"\\n\", scalar(@ARGV), \"\\n\"; close(F); sleep $((31500 + n))" >"$d/prog.pl"
  local quoted
  case "$name" in
    fish) quoted="'it\\'s \$HOME;\`id\` \\'q\\' \"d\" -n'" ;;
    nu) quoted="r#'it's \$HOME;\`id\` 'q' \"d\" -n'#" ;;
    pwsh) quoted="'it''s \$HOME;\`id\` ''q'' \"d\" -n'" ;;
    *) quoted="'it'\\''s \$HOME;\`id\` '\\''q'\\'' \"d\" -n'" ;;
  esac
  $BIN new-session -d -s r >"$d/o3" 2>&1
  sleep 5
  $BIN send-keys -t r: -l "perl $d/prog.pl $quoted" >>"$d/o3" 2>&1
  $BIN send-keys -t r: Enter >>"$d/o3" 2>&1
  assert "$name: the program runs" "until_true '[ \"\$(lines_of \"$d/args.txt\")\" = 2 ]' 30"
  assert "$name: it got the hostile word as exactly one argument when typed by hand" \
    '[ "$(sed -n 1p "$d/args.txt")" = "$HOSTILE" ] && [ "$(sed -n 2p "$d/args.txt")" = 1 ]'
  $BIN save snap >"$d/o4" 2>&1
  pkill -f "$d/prog.pl" 2>/dev/null
  $BIN kill-session -t r >/dev/null 2>&1
  $BIN restore snap >"$d/o5" 2>&1
  assert "$name: it is typed again after the restore" "until_true 'pgrep -f \"$d/prog.pl\" >/dev/null' 60"
  assert "$name: the second run wrote its arguments" "until_true '[ \"\$(lines_of \"$d/args.txt\")\" = 4 ]' 20"
  assert "$name: both runs saw the hostile word as exactly one argument" \
    '[ "$(sed -n 1p "$d/args.txt")" = "$HOSTILE" ] && [ "$(sed -n 2p "$d/args.txt")" = 1 ] && [ "$(sed -n 3p "$d/args.txt")" = "$HOSTILE" ] && [ "$(sed -n 4p "$d/args.txt")" = 1 ]'
  pkill -f "$d/prog.pl" 2>/dev/null

  $BIN kill-server >/dev/null 2>&1
  wait "$client" 2>/dev/null
  export TMPDIR="$old_tmp"
}

(sleep 900; for s in "$TMPDIR"/*/sock; do TMPDIR="$s" $BIN kill-server >/dev/null 2>&1; done) >/dev/null 2>&1 &
watchdog=$!

n=1
for shell in ${SHELLS:-bash zsh fish nu pwsh}; do
  path=$(command -v "$shell" || true)
  if [ -z "$path" ]; then
    echo "skip $shell: not installed"
    continue
  fi
  one_shell "$shell" "$path" "$n"
  n=$((n + 1))
done

kill "$watchdog" 2>/dev/null
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
