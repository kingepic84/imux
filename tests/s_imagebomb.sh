#!/bin/bash
# A program in a pane that prints many small, hugely expanding pictures (a 4 KB PNG that decodes to
# 31 Mpx, kitty and iTerm2 style) must not stop the server from answering: the pictures are decoded
# on a worker thread and only that pane waits. Before the worker, 400 of them kept the server from
# answering for half a minute. Also checks that the pane carries on afterwards and that the
# pictures were drawn, and that another pane is not held up.
set -u
BIN="${1:-./target/debug/imux}"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BIN
export TMPDIR
TMPDIR=$(mktemp -d /tmp/imux-sib.XXXXXX)
export IMUX_STATE_DIR="$TMPDIR/state"
export IMUX_CONFIG="$TMPDIR/imux.conf"
export SHELL=/bin/sh
unset IMUX_ENV IMUX_PANE_ID IMUX_SOCKET_PATH IMUX_SERVER_SOCKET IMUX_SOCKET_NAME
fail=0

assert() {
  if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi
}

(sleep 400; $BIN kill-server >/dev/null 2>&1; kill $$ >/dev/null 2>&1) >/dev/null 2>&1 &

now_ms() {
  perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'
}

printf 'set -g status off\nset -g default-size 100x30\n' >"$IMUX_CONFIG"

# 30 pictures, half iTerm2 and half kitty: a 5600 x 5600 one-bit PNG is about 4 KB.
python3 - "$TMPDIR/flood.bin" <<'EOF'
import base64, struct, sys, zlib
w = h = 5600
def chunk(t, d):
    c = struct.pack('>I', len(d)) + t + d
    return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
raw = (b'\x00' + bytes((w + 7) // 8)) * h
png = (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 1, 0, 0, 0, 0))
       + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))
b64 = base64.b64encode(png)
out = bytearray()
for i in range(30):
    if i % 2:
        out += b'\x1b_Ga=T,f=100,c=4,r=2;' + b64 + b'\x1b\\'
    else:
        out += b'\x1b]1337;File=inline=1;width=4;height=2:' + b64 + b'\x07'
    out += b'\r\n'
sys.stdout = open(sys.argv[1], 'wb')
sys.stdout.write(bytes(out))
EOF

$BIN kill-server >/dev/null 2>&1
$BIN new-session -d -s bomb >/dev/null 2>&1
$BIN new-window -d -t bomb -n calm >/dev/null 2>&1
sleep 1
$BIN send-keys -t bomb:0 -l "cat $TMPDIR/flood.bin; echo BOMBDONE-\$((6*7))" >/dev/null 2>&1
$BIN send-keys -t bomb:0 Enter >/dev/null 2>&1

echo "-- the server answers while the pictures are decoded"
worst=0
samples=0
end=$(( $(now_ms) + 20000 ))
while [ "$(now_ms)" -lt "$end" ]; do
  s=$(now_ms)
  timeout 10 $BIN ls >/dev/null 2>&1
  ms=$(( $(now_ms) - s ))
  [ "$ms" -gt "$worst" ] && worst=$ms
  samples=$((samples + 1))
  $BIN capture-pane -p -t bomb:0 2>/dev/null | grep -aq BOMBDONE-42 && break
  sleep 0.2
done
echo "     $samples answers, the slowest took ${worst} ms"
assert "no answer took longer than two seconds" '[ "$worst" -lt 2000 ]'

echo "-- another pane is not held up"
$BIN send-keys -t bomb:calm -l "echo CALMPANE-\$((6*7))" >/dev/null 2>&1
$BIN send-keys -t bomb:calm Enter >/dev/null 2>&1
ok=0
for i in $(seq 1 25); do
  $BIN capture-pane -p -t bomb:calm 2>/dev/null | grep -aq "CALMPANE-42" && ok=1 && break
  sleep 0.2
done
assert "a shell in another pane still answers" '[ $ok = 1 ]'

echo "-- the pane carries on after the pictures"
done_seen=0
for i in $(seq 1 600); do
  $BIN capture-pane -p -t bomb:0 2>/dev/null | grep -aq BOMBDONE-42 && done_seen=1 && break
  sleep 0.5
done
assert "the text after the last picture arrives" '[ $done_seen = 1 ]'
assert "the pictures were drawn" '$BIN capture-pane -p -t bomb:0 2>/dev/null | grep -aq "\[image\]"'

$BIN kill-server >/dev/null 2>&1
if [ "$fail" = 0 ]; then rm -rf "$TMPDIR"; else echo "kept $TMPDIR for inspection"; fi
exit $fail
