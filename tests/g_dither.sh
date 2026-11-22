#!/bin/bash
# image-dither: a picture with more colors than the palette has, drawn for a Sixel client and for
# a client that gets 256-color blocks. Off is what it always was; ordered changes the bytes; a
# client with 24-bit color is not touched; the option is read.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
HERE="$(cd "$(dirname "$0")" && pwd)"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-gd.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
ESC=$(printf '\033')

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 300; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

B64=$(base64 <"$HERE/fixtures/dither_gradient.png" | tr -d '\n')
printf '%s\n' "printf '\\033]1337;File=inline=1;width=40;height=12;preserveAspectRatio=0:%s\\007' '$B64'" "echo marker-after-picture" >"$TMPDIR/pic.sh"

run_with() {
  local name=$1 term=$2 conf=$3
  printf '%s\n' "set image-probe always" "$conf" >"$IMUX_CONFIG"
  (sleep 1; printf '%s\r' "sh $TMPDIR/pic.sh"; sleep 3; printf '\002d'; sleep 1) \
    | timeout 40 script -q "$TMPDIR/$name.out" sh -c "stty rows 30 cols 100; env -u LC_TERMINAL -u COLORTERM -u TERM_PROGRAM -u KITTY_WINDOW_ID $term $BIN new -s gd" >/dev/null
  $BIN kill-server >/dev/null 2>&1
  sleep 1
}

first_sixel() {
  perl -0777 -ne 'print $1 if /(\eP0;1;0q.*?\e\\)/s' "$1" 2>/dev/null
}

indices() {
  LC_ALL=C grep -ao '[34]8;5;[0-9]*' "$1" | sort -u | wc -l | tr -d ' '
}

echo "-- Sixel"
run_with six_off "TERM=foot" "set image-mode sixel"
run_with six_off2 "TERM=foot" "set image-mode sixel"
run_with six_on "TERM=foot" "set image-mode sixel
set image-dither ordered"
run_with six_on2 "TERM=foot" "set image-mode sixel
set image-dither ordered"
first_sixel "$TMPDIR/six_off.out" >"$TMPDIR/s_off"
first_sixel "$TMPDIR/six_off2.out" >"$TMPDIR/s_off2"
first_sixel "$TMPDIR/six_on.out" >"$TMPDIR/s_on"
first_sixel "$TMPDIR/six_on2.out" >"$TMPDIR/s_on2"
assert "a picture is drawn as Sixel with the dither off" '[ -s "$TMPDIR/s_off" ]'
assert "and with it on" '[ -s "$TMPDIR/s_on" ]'
assert "off gives the same bytes every time" 'cmp -s "$TMPDIR/s_off" "$TMPDIR/s_off2"'
assert "ordered gives the same bytes every time" 'cmp -s "$TMPDIR/s_on" "$TMPDIR/s_on2"'
assert "ordered changes the picture" '! cmp -s "$TMPDIR/s_off" "$TMPDIR/s_on"'
assert "the text after the picture still arrives" 'LC_ALL=C grep -aq marker-after-picture "$TMPDIR/six_on.out"'

echo "-- 256-color blocks"
run_with blk_off "TERM_PROGRAM=Apple_Terminal" ""
run_with blk_on "TERM_PROGRAM=Apple_Terminal" "set image-dither ordered"
assert "blocks use palette colors without the dither" '[ "$(indices "$TMPDIR/blk_off.out")" -gt 0 ]'
assert "no escape for a picture protocol" '! LC_ALL=C grep -aqF "${ESC}P0;1;0q" "$TMPDIR/blk_on.out"'
assert "ordered uses more palette entries across the same gradient" '[ "$(indices "$TMPDIR/blk_on.out")" -gt "$(indices "$TMPDIR/blk_off.out")" ]'

echo "-- a client with 24-bit color is left alone"
run_with tc_off "TERM_PROGRAM=iTerm.app COLORTERM=truecolor" "set image-mode blocks"
run_with tc_on "TERM_PROGRAM=iTerm.app COLORTERM=truecolor" "set image-mode blocks
set image-dither ordered"
assert "truecolor blocks are drawn as 24-bit color" 'LC_ALL=C grep -aq "38;2;" "$TMPDIR/tc_on.out" || LC_ALL=C grep -aq "48;2;" "$TMPDIR/tc_on.out"'
assert "with no palette colors" '[ "$(indices "$TMPDIR/tc_on.out")" = 0 ]'
assert "and the same set of colors as without the dither" '[ "$(LC_ALL=C grep -ao "[34]8;2;[0-9;]*" "$TMPDIR/tc_off.out" | sort -u | md5)" = "$(LC_ALL=C grep -ao "[34]8;2;[0-9;]*" "$TMPDIR/tc_on.out" | sort -u | md5)" ]'

echo "-- the option"
printf 'set image-dither sometimes\n' >"$TMPDIR/bad.conf"
IMUX_CONFIG="$TMPDIR/bad.conf" $BIN doctor >"$TMPDIR/d1" 2>&1
assert "a wrong value is reported with the choices" 'grep -aq "expected one of none, ordered" "$TMPDIR/d1"'
IMUX_CONFIG=/dev/null $BIN new-session -d -s o >/dev/null 2>&1
$BIN show-options >"$TMPDIR/o1" 2>&1
assert "the default is none" 'grep -aq "^image-dither none$" "$TMPDIR/o1"'
$BIN kill-server >/dev/null 2>&1

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
