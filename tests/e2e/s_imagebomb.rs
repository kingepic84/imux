//! A program in a pane that prints many small, hugely expanding pictures (a 4 KB PNG that decodes to
//! 31 Mpx, kitty and iTerm2 style) must not stop the server from answering: the pictures are decoded
//! on a worker thread and only that pane waits. Before the worker, 400 of them kept the server from
//! answering for half a minute. Also checks that the pane carries on afterwards and that the
//! pictures were drawn, and that another pane is not held up. (The Rust version of tests/s_imagebomb.sh.)
use crate::support::*;
use std::time::{Duration, Instant};

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut body = kind.to_vec();
    body.extend_from_slice(data);
    v.extend_from_slice(&body);
    v.extend_from_slice(&crc32(&body).to_be_bytes());
    v
}

fn base64(input: &[u8]) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len() / 3 * 4 + 4);
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63]);
        out.push(T[(n >> 12) as usize & 63]);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] } else { b'=' });
        out.push(if c.len() > 2 { T[n as usize & 63] } else { b'=' });
    }
    out
}

/// 30 pictures, half iTerm2 and half kitty: a 5600 x 5600 one-bit PNG is about 4 KB.
fn flood() -> Vec<u8> {
    let (w, h) = (5600u32, 5600usize);
    let row = {
        let mut r = vec![0u8]; // filter byte
        r.extend(std::iter::repeat_n(0u8, (w as usize).div_ceil(8)));
        r
    };
    let raw: Vec<u8> = row.iter().copied().cycle().take(row.len() * h).collect();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[1, 0, 0, 0, 0]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(chunk(b"IHDR", &ihdr));
    png.extend(chunk(b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9)));
    png.extend(chunk(b"IEND", b""));
    let b64 = base64(&png);
    let mut out = Vec::new();
    for i in 0..30 {
        if i % 2 == 1 {
            out.extend_from_slice(b"\x1b_Ga=T,f=100,c=4,r=2;");
            out.extend_from_slice(&b64);
            out.extend_from_slice(b"\x1b\\");
        } else {
            out.extend_from_slice(b"\x1b]1337;File=inline=1;width=4;height=2:");
            out.extend_from_slice(&b64);
            out.push(7);
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[test]
fn s_imagebomb() {
    let mut e = Env::new("sib");
    let conf = e.write("imux.conf", "set -g status off\nset -g default-size 100x30\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("s_imagebomb");
    let bin = e.path("flood.bin");
    std::fs::write(&bin, flood()).unwrap();

    e.session("bomb", "w");
    e.run(&["new-window", "-d", "-t", "bomb", "-n", "calm"]);
    assert!(e.ready("bomb:0", Duration::from_secs(1)), "the shell of bomb:0 never settled");
    e.run(&["send-keys", "-t", "bomb:0", "-l", &format!("cat {}; echo BOMBDONE-$((6*7))", p(&bin))]);
    e.run(&["send-keys", "-t", "bomb:0", "Enter"]);

    t.section("the server answers while the pictures are decoded");
    let mut worst = 0u128;
    let mut samples = 0;
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        let s = Instant::now();
        e.run_for(&["ls"], Duration::from_secs(10));
        worst = worst.max(s.elapsed().as_millis());
        samples += 1;
        if e.capture("bomb:0").contains("BOMBDONE-42") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("     {samples} answers, the slowest took {worst} ms");
    t.ok("no answer took longer than two seconds", worst < 2000);

    t.section("another pane is not held up");
    e.run(&["send-keys", "-t", "bomb:calm", "-l", "echo CALMPANE-$((6*7))"]);
    e.run(&["send-keys", "-t", "bomb:calm", "Enter"]);
    t.ok("a shell in another pane still answers", wait_until(Duration::from_secs(5), || e.capture("bomb:calm").contains("CALMPANE-42")));

    t.section("the pane carries on after the pictures");
    t.ok("the text after the last picture arrives", wait_until(Duration::from_secs(300), || e.capture("bomb:0").contains("BOMBDONE-42")));
    t.ok("the pictures were drawn", e.capture("bomb:0").contains("[image]"));
    t.finish();
}
