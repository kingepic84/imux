//! image-dither: a picture with more colors than the palette has, drawn for a Sixel client and for
//! a client that gets 256-color blocks. Off is what it always was; ordered changes the bytes; a
//! client with 24-bit color is not touched; the option is read. (The Rust version of tests/g_dither.sh.)
use crate::support::*;
use regex_lite::Regex;
use std::collections::BTreeSet;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

fn terminal(term: &str) -> Vec<(String, String)> {
    term.split_whitespace().filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// One run: a client of the given terminal draws the picture; what it wrote is returned.
fn run_with(e: &Env, term: &str, conf: &str) -> Vec<u8> {
    e.write("imux.conf", &format!("set image-probe always\n{conf}\n"));
    let vars = terminal(term);
    let set: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut c = e.client_env(100, 30, &["new", "-s", "gd"], &set, &["LC_TERMINAL", "COLORTERM", "TERM_PROGRAM", "KITTY_WINDOW_ID"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("gd:", Duration::from_millis(600)), "the pane never settled");
    c.send_str(&format!("sh {}\r", p(&e.path("pic.sh"))));
    assert!(wait_until(LONG, || e.capture("gd:").lines().any(|l| l == "marker-after-picture")), "the picture script did not finish");
    e.barrier(&c, "gd");
    c.send(b"\x02d");
    c.wait_exit(Duration::from_secs(10));
    let bytes = c.bytes();
    e.kill_server();
    bytes
}

/// The first Sixel stream of the output, ESC P 0;1;0 q ... ESC \.
fn first_sixel(bytes: &[u8]) -> Vec<u8> {
    let start: &[u8] = b"\x1bP0;1;0q";
    let Some(i) = bytes.windows(start.len()).position(|w| w == start) else { return Vec::new() };
    let rest = &bytes[i..];
    let end = rest.windows(2).position(|w| w == b"\x1b\\").map(|j| j + 2).unwrap_or(rest.len());
    rest[..end].to_vec()
}

fn matches_in(bytes: &[u8], pattern: &str) -> BTreeSet<String> {
    let re = Regex::new(pattern).unwrap();
    let text = String::from_utf8_lossy(bytes);
    re.find_iter(&text).map(|m| m.as_str().to_string()).collect()
}

fn indices(bytes: &[u8]) -> usize {
    matches_in(bytes, "[34]8;5;[0-9]*").len()
}

#[test]
fn g_dither() {
    let mut e = Env::new("gdi");
    let conf = p(&e.path("imux.conf"));
    e.set("IMUX_CONFIG", &conf);
    e.unset("SHELL");
    let png = std::fs::read(format!("{}/tests/fixtures/dither_gradient.png", env!("CARGO_MANIFEST_DIR"))).expect("the fixture");
    let b64 = base64_encode(&png);
    e.write("pic.sh", &format!("printf '\\033]1337;File=inline=1;width=40;height=12;preserveAspectRatio=0:%s\\007' '{b64}'\necho marker-after-picture\n"));
    let mut t = Check::new("g_dither");

    t.section("Sixel");
    let six_off = run_with(&e, "TERM=foot", "set image-mode sixel");
    let six_off2 = run_with(&e, "TERM=foot", "set image-mode sixel");
    let six_on = run_with(&e, "TERM=foot", "set image-mode sixel\nset image-dither ordered");
    let six_on2 = run_with(&e, "TERM=foot", "set image-mode sixel\nset image-dither ordered");
    let (s_off, s_off2, s_on, s_on2) = (first_sixel(&six_off), first_sixel(&six_off2), first_sixel(&six_on), first_sixel(&six_on2));
    t.ok("a picture is drawn as Sixel with the dither off", !s_off.is_empty());
    t.ok("and with it on", !s_on.is_empty());
    t.ok("off gives the same bytes every time", s_off == s_off2);
    t.ok("ordered gives the same bytes every time", s_on == s_on2);
    t.ok("ordered changes the picture", s_off != s_on);
    t.ok("the text after the picture still arrives", String::from_utf8_lossy(&six_on).contains("marker-after-picture"));

    t.section("256-color blocks");
    let blk_off = run_with(&e, "TERM_PROGRAM=Apple_Terminal", "");
    let blk_on = run_with(&e, "TERM_PROGRAM=Apple_Terminal", "set image-dither ordered");
    t.ok("blocks use palette colors without the dither", indices(&blk_off) > 0);
    t.ok("no escape for a picture protocol", !blk_on.windows(8).any(|w| w == b"\x1bP0;1;0q"));
    t.ok("ordered uses more palette entries across the same gradient", indices(&blk_on) > indices(&blk_off));

    t.section("a client with 24-bit color is left alone");
    let tc_off = run_with(&e, "TERM_PROGRAM=iTerm.app COLORTERM=truecolor", "set image-mode blocks");
    let tc_on = run_with(&e, "TERM_PROGRAM=iTerm.app COLORTERM=truecolor", "set image-mode blocks\nset image-dither ordered");
    let on = String::from_utf8_lossy(&tc_on);
    t.ok("truecolor blocks are drawn as 24-bit color", on.contains("38;2;") || on.contains("48;2;"));
    t.ok("with no palette colors", indices(&tc_on) == 0);
    t.ok("and the same set of colors as without the dither", matches_in(&tc_off, "[34]8;2;[0-9;]*") == matches_in(&tc_on, "[34]8;2;[0-9;]*"));

    t.section("the option");
    let bad = e.write("bad.conf", "set image-dither sometimes\n");
    let r = e.run_env(&["doctor"], &[("IMUX_CONFIG", &p(&bad))]);
    t.ok("a wrong value is reported with the choices", r.all().contains("expected one of none, ordered"));
    e.run_env(&["new-session", "-d", "-s", "o"], &[("IMUX_CONFIG", "/dev/null")]);
    t.matches("the default is none", "^image-dither none$", &e.all(&["show-options"]));
    t.finish();
}
