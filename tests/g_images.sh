#!/bin/bash
# Picture output as the image scrolling work left it: a Sixel stream sent as an iTerm2 file (what
# imgcat does with a .six file), a forced image-mode that is tried first, the Kitty placement by id,
# the glyph sets of image-glyphs, and the fallback to the normal chain.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-g.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
fail=0
ESC=$(printf '\033')

check() {
  if LC_ALL=C grep -aqF -- "$2" "$3"; then echo "ok   $1"; else echo "FAIL $1 (missing: $2)"; fail=1; fi
}

refute() {
  if LC_ALL=C grep -aqF -- "$2" "$3"; then echo "FAIL $1 (found: $2)"; fail=1; else echo "ok   $1"; fi
}

(sleep 200; $BIN kill-server >/dev/null 2>&1) >/dev/null 2>&1 &

SIXEL=$(printf '\033Pq"1;1;2;6#0;2;100;0;0#0B?\033\\' | base64 | tr -d '\n')
cat >"$TMPDIR/six.sh" <<EOF
printf '\033]1337;File=inline=1;width=1;height=1;preserveAspectRatio=0:%s\007' '$SIXEL'
echo marker-after-six
EOF

run_client() {
  local out=$1 term=$2
  (sleep 1; printf '%s\r' "sh $TMPDIR/six.sh"; sleep 2; printf '%s\r' "sh $TMPDIR/six.sh"; sleep 2; printf '\002d'; sleep 1) \
    | timeout 40 script -q "$out" sh -c "stty rows 30 cols 100; env -u LC_TERMINAL -u COLORTERM -u TERM_PROGRAM -u KITTY_WINDOW_ID $term $BIN new -s g" >/dev/null
}

run_with() {
  local name=$1 term=$2 conf=$3
  printf '%s\n' "set image-probe always" "$conf" >"$IMUX_CONFIG"
  run_client "$TMPDIR/$name.out" "$term"
  $BIN kill-server >/dev/null 2>&1
  sleep 1
}

echo "-- a Sixel stream sent as an iTerm2 file"
run_with six_iterm "TERM_PROGRAM=iTerm.app" ""
check "an iterm2 client gets the picture as an inline image" "${ESC}]1337;File=" "$TMPDIR/six_iterm.out"
check "and the text after it still arrives" "marker-after-six" "$TMPDIR/six_iterm.out"

echo "-- a forced image-mode is tried first"
run_with forced_sixel "TERM_PROGRAM=iTerm.app" "set image-mode sixel"
check "image-mode sixel gives an iterm2 client Sixel" "${ESC}P0;1;0q" "$TMPDIR/forced_sixel.out"
refute "and no inline image" "${ESC}]1337;File=" "$TMPDIR/forced_sixel.out"
run_with forced_kitty "TERM_PROGRAM=Apple_Terminal" "set image-mode kitty"
check "image-mode kitty gives any client Kitty graphics" "${ESC}_Ga=t,i=1," "$TMPDIR/forced_kitty.out"
check "placed by id, a source rectangle" "${ESC}_Ga=p,i=1,p=1," "$TMPDIR/forced_kitty.out"
refute "and no Sixel" "${ESC}P0;1;0q" "$TMPDIR/forced_kitty.out"
run_with forced_iterm "TERM=foot" "set image-mode iterm2"
check "image-mode iterm2 gives a Sixel terminal the inline image" "${ESC}]1337;File=" "$TMPDIR/forced_iterm.out"
refute "and no Sixel" "${ESC}P0;1;0q" "$TMPDIR/forced_iterm.out"

echo "-- without image-mode the order is iTerm2, Kitty, Sixel, blocks"
run_with auto_kitty "TERM=xterm-kitty" "set image-mode auto"
check "a Kitty terminal gets Kitty" "${ESC}_Ga=t,i=1," "$TMPDIR/auto_kitty.out"
run_with auto_sixel "TERM=foot" ""
check "a Sixel terminal gets Sixel" "${ESC}P0;1;0q" "$TMPDIR/auto_sixel.out"
run_with auto_blocks "TERM_PROGRAM=Apple_Terminal" ""
refute "Terminal.app gets no picture protocol" "${ESC}P0;1;0q" "$TMPDIR/auto_blocks.out"
refute "not Kitty either" "${ESC}_Ga=" "$TMPDIR/auto_blocks.out"
refute "not an inline image" "${ESC}]1337;File=" "$TMPDIR/auto_blocks.out"

echo "-- image-kitty-placement crop stores each crop"
run_with kitty_crop "TERM=xterm-kitty" "set image-kitty-placement crop"
check "crop mode still transmits and places" "${ESC}_Ga=t,i=1," "$TMPDIR/kitty_crop.out"

echo "-- image-glyphs"
run_with glyph_quadrant "TERM_PROGRAM=Apple_Terminal" ""
run_with glyph_sextant "TERM_PROGRAM=Apple_Terminal" "set image-glyphs sextant"
run_with glyph_octant "TERM_PROGRAM=Apple_Terminal" "set image-glyphs octant"
SEXTANT_LEAD=$(printf '\360\237\254')
OCTANT_LEAD=$(printf '\360\234')
check "quadrants draw the picture at all" "38;5;196m" "$TMPDIR/glyph_quadrant.out"
refute "quadrants use no sextant characters" "$SEXTANT_LEAD" "$TMPDIR/glyph_quadrant.out"
refute "quadrants use no octant characters" "$OCTANT_LEAD" "$TMPDIR/glyph_quadrant.out"
check "sextants are drawn when asked for" "$SEXTANT_LEAD" "$TMPDIR/glyph_sextant.out"
check "octants are drawn when asked for" "$OCTANT_LEAD" "$TMPDIR/glyph_octant.out"

if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
