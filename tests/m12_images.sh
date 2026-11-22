#!/bin/bash
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-img.XXXXXX)
export IMUX_CONFIG=/dev/null
export IMUX_STATE_DIR="$TMPDIR/state"
fail=0

ESC=$(printf '\033')
OSC="${ESC}]1337;File=inline=1;width=4;height=2;preserveAspectRatio=0:"

check() {
  if grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if grep -aqF -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

PNG='R0lGODlhAQABAIAAAP8AAAAAACwAAAAAAQABAAACAkQBADs='
cat >"$TMPDIR/img.sh" <<EOF
printf '\033]1337;File=inline=1;width=4;height=2;preserveAspectRatio=0:%s\007' '$PNG'
echo marker-after-image
EOF

run_client() {
  local out=$1 env=$2
  (sleep 1; printf '%s\r' "sh $TMPDIR/img.sh"; sleep 2; printf '%s\r' "sh $TMPDIR/img.sh"; sleep 2) \
    | timeout 60 script -q "$out" sh -c "stty rows 30 cols 100; env -u LC_TERMINAL -u COLORTERM -u TERM_PROGRAM -u KITTY_WINDOW_ID $env $BIN new -s img" >/dev/null
}

(sleep 40; $BIN kill-server >/dev/null 2>&1) &

run_client "$TMPDIR/iterm.out" "TERM_PROGRAM=iTerm.app"
$BIN kill-server >/dev/null 2>&1
sleep 1
printf 'set image-probe always\n' >"$TMPDIR/probe.conf"
run_client "$TMPDIR/apple.out" "TERM_PROGRAM=Apple_Terminal IMUX_CONFIG=$TMPDIR/probe.conf"
$BIN kill-server >/dev/null 2>&1
sleep 1
run_client "$TMPDIR/kitty.out" "TERM=xterm-kitty"
$BIN kill-server >/dev/null 2>&1
sleep 1
run_client "$TMPDIR/sixel.out" "TERM=foot"
$BIN kill-server >/dev/null 2>&1

check "kitty client gets a graphics command" "${ESC}_Ga=t,i=1,f=100,t=d,q=2" "$TMPDIR/kitty.out"
check "kitty client places the stored picture by id" "${ESC}_Ga=p,i=1,p=1," "$TMPDIR/kitty.out"
refute "kitty client gets no iterm2 image" "${ESC}]1337;File=" "$TMPDIR/kitty.out"
check "sixel client gets a sixel image" "${ESC}P0;1;0q" "$TMPDIR/sixel.out"
refute "sixel client gets no kitty command" "${ESC}_Ga=" "$TMPDIR/sixel.out"
check "every client asks the terminal what it supports" "${ESC}_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA${ESC}\\${ESC}[c" "$TMPDIR/apple.out"
check "iterm2 client gets an inline image" "$OSC" "$TMPDIR/iterm.out"
check "iterm2 client: text after the image still arrives" "marker-after-image" "$TMPDIR/iterm.out"
refute "terminal.app client gets no inline image" "${ESC}]1337;File=" "$TMPDIR/apple.out"
check "terminal.app client: text after the image still arrives" "marker-after-image" "$TMPDIR/apple.out"
check "terminal.app client gets 256-color backgrounds for the picture" "48;5;" "$TMPDIR/apple.out"
refute "terminal.app client gets no truecolor" "38;2;" "$TMPDIR/apple.out"

exit $fail
