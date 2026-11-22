#!/bin/bash
# The gaps that group C closed: the terminal query (image-probe), text for pictures in copies,
# nested programs in a snapshot, joined emoji (emoji-width), DCS replies, and the Kitty extras
# (delete targets, placement ids, media files, unicode placeholders).
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-c.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
ESC=$(printf '\033')

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aqF -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

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

(sleep 300; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

run_client() {
  (sleep 2; printf 'echo up-$((1+2))\r'; sleep 1.5; printf '\002d'; sleep 1) \
    | timeout 40 script -q "$1" sh -c "stty rows 30 cols 100; env -u LC_TERMINAL -u COLORTERM -u KITTY_WINDOW_ID TERM_PROGRAM=$2 $BIN new -s p" >/dev/null
}

echo "-- image-probe"
printf '' >"$IMUX_CONFIG"
run_client "$TMPDIR/apple.out" Apple_Terminal
$BIN kill-server >/dev/null 2>&1
sleep 1
refute "Terminal.app is not asked by default" "${ESC}_Gi=31" "$TMPDIR/apple.out"
check "and the session works" "up-3" "$TMPDIR/apple.out"
printf 'set image-probe always\n' >"$IMUX_CONFIG"
run_client "$TMPDIR/always.out" Apple_Terminal
$BIN kill-server >/dev/null 2>&1
sleep 1
check "image-probe always asks it anyway" "${ESC}_Gi=31" "$TMPDIR/always.out"
printf 'set image-probe never\n' >"$IMUX_CONFIG"
run_client "$TMPDIR/never.out" iTerm.app
$BIN kill-server >/dev/null 2>&1
sleep 1
refute "image-probe never asks nobody" "${ESC}_Gi=31" "$TMPDIR/never.out"
printf '' >"$IMUX_CONFIG"

cat >"$TMPDIR/pic.sh" <<'EOF'
printf 'A '
printf '\033_Ga=T,f=24,s=1,v=1,c=3,r=1,i=1,q=2;/wAA\033\\'
printf 'B '
printf '\033_Ga=T,f=24,s=1,v=1,c=3,r=1,i=2,q=2;/wAA\033\\'
printf '\033_Ga=d,d=i,i=2\033\\'
printf 'C '
printf '\033_Ga=T,U=1,f=24,s=1,v=1,c=2,r=1,i=7,q=2;/wAA\033\\'
printf '\033[38;5;7m\364\216\273\256\314\205\314\205\364\216\273\256\033[0m\n'
printf 'D '
printf '\033_Ga=T,t=f,f=24,s=1,v=1,i=9,q=2;%s\033\\\n' "$(printf '%s' "$PIXFILE" | base64)"
touch "$1"
EOF

printf '\377\000\000' >"$TMPDIR/pixel.rgb"
export PIXFILE="$TMPDIR/pixel.rgb"

$BIN new-session -d -s c >/dev/null 2>&1
$BIN send-keys -t c:0 "PIXFILE=$PIXFILE sh $TMPDIR/pic.sh $TMPDIR/pic.done" Enter
until_true '[ -f "$TMPDIR/pic.done" ]' 40
sleep 1
$BIN capture-pane -p -t c:0 >"$TMPDIR/cap1.txt" 2>&1

echo "-- pictures in copied text, delete targets, placeholders, media"
check "a picture is [image] in capture-pane" "A [image]" "$TMPDIR/cap1.txt"
refute "a deleted picture (d=i) leaves nothing" "B [image]" "$TMPDIR/cap1.txt"
check "the row of a deleted picture still has its text" "B" "$TMPDIR/cap1.txt"
check "placeholder characters show the picture once per run" "C [image]" "$TMPDIR/cap1.txt"
refute "a placeholder character does not show as text" "$(printf '\364\216\273\256')" "$TMPDIR/cap1.txt"
refute "a file is not read while kitty-media is none" "D [image]" "$TMPDIR/cap1.txt"

$BIN set -g image-copy-text '<pic>' >/dev/null 2>&1
$BIN capture-pane -p -t c:0 >"$TMPDIR/cap2.txt" 2>&1
check "image-copy-text changes the text" "A <pic>" "$TMPDIR/cap2.txt"
$BIN set -g image-copy-text '' >/dev/null 2>&1
$BIN capture-pane -p -t c:0 >"$TMPDIR/cap3.txt" 2>&1
refute "and an empty one leaves the picture out" "[image]" "$TMPDIR/cap3.txt"
$BIN set -g image-copy-text '[image]' >/dev/null 2>&1

$BIN set -g kitty-media files >/dev/null 2>&1
rm -f "$TMPDIR/pic.done"
$BIN send-keys -t c:0 "clear; PIXFILE=$PIXFILE sh $TMPDIR/pic.sh $TMPDIR/pic.done" Enter
until_true '[ -f "$TMPDIR/pic.done" ]' 40
sleep 1
$BIN capture-pane -p -t c:0 >"$TMPDIR/cap4.txt" 2>&1
check "with kitty-media files the file picture is shown" "D [image]" "$TMPDIR/cap4.txt"
$BIN set -g kitty-media none >/dev/null 2>&1

cat >"$TMPDIR/probe.sh" <<'EOF'
stty raw -echo
printf "$3"
dd bs=1 count="$2" of="$1" 2>/dev/null
stty sane
touch "$1.done"
EOF

reply_of() {
  rm -f "$TMPDIR/reply.bin" "$TMPDIR/reply.bin.done"
  $BIN send-keys -t c:0 "clear; sh $TMPDIR/probe.sh $TMPDIR/reply.bin $2 '$1'" Enter
  until_true '[ -f "$TMPDIR/reply.bin.done" ]' 40
}

echo "-- DCS replies"
reply_of '\033P+q544e\033\\' 20
assert "XTGETTCAP TN is answered with a fixed value" '[ "$(od -An -c "$TMPDIR/reply.bin" | tr -d " \n")" = "033P1+r544E=696D7578033\\" ]'
reply_of '\033P$qsecret\033\\' 7
assert "an unknown setting request is an invalid request without an echo" '[ "$(od -An -c "$TMPDIR/reply.bin" | tr -d " \n")" = "033P0\$r033\\" ]'

echo "-- emoji-width"
FAMILY='\360\237\221\250\342\200\215\360\237\221\251\342\200\215\360\237\221\247'
reply_of "$FAMILY"'\033[6n' 6
assert "by default the three people take six cells (column 7)" '[ "$(od -An -c "$TMPDIR/reply.bin" | tr -d " \n")" = "033[1;7R" ]'
$BIN set -g emoji-width cluster >/dev/null 2>&1
reply_of "$FAMILY"'\033[6n' 6
assert "with emoji-width cluster they are one glyph of two cells (column 3)" '[ "$(od -An -c "$TMPDIR/reply.bin" | tr -d " \n")" = "033[1;3R" ]'
$BIN set -g emoji-width parts >/dev/null 2>&1

echo "-- a program inside a nested shell"
$BIN kill-server >/dev/null 2>&1
sleep 1
printf 'set restore-commands "sleep"\n' >"$IMUX_CONFIG"
$BIN new-session -d -s n >/dev/null 2>&1
$BIN send-keys -t n:0 "sh" Enter
sleep 1
$BIN send-keys -t n:0 "sleep 317" Enter
sleep 1.5
$BIN save nested >/dev/null 2>&1
assert "the snapshot records the shell the program runs in" 'find "$IMUX_STATE_DIR/snapshots" -type f | xargs grep -al "+outer 1 1 sh" | grep -q .'
$BIN kill-server >/dev/null 2>&1
sleep 1.5
$BIN new-session -d -s other >/dev/null 2>&1
$BIN restore nested >/dev/null 2>&1
until_true '$BIN list-panes -a -F "#{pane_current_command}" 2>/dev/null | grep -qx sleep' 40
assert "restore types the shell and then the program" '$BIN list-panes -a -F "#{pane_current_command}" 2>/dev/null | grep -qx sleep'
$BIN kill-server >/dev/null 2>&1

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
