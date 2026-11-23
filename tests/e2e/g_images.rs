//! Picture output as the image scrolling work left it: a Sixel stream sent as an iTerm2 file (what
//! imgcat does with a .six file), a forced image-mode that is tried first, the Kitty placement by id,
//! the glyph sets of image-glyphs, and the fallback to the normal chain.
//! (The Rust version of tests/g_images.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);
const ESC: &str = "\x1b";

/// One run: a client of the given terminal draws the Sixel picture twice; what it wrote is returned.
fn run_with(e: &Env, term: &[(&str, &str)], conf: &str) -> Client {
    e.write("imux.conf", &format!("set image-probe always\n{conf}\n"));
    let mut c = e.client_env(100, 30, &["new", "-s", "g"], term, &["LC_TERMINAL", "COLORTERM", "TERM_PROGRAM", "KITTY_WINDOW_ID"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("g:", Duration::from_millis(600)), "the pane never settled");
    let six = p(&e.path("six.sh"));
    c.send_str(&format!("sh {six}\r"));
    assert!(wait_until(LONG, || e.capture("g:").matches("marker-after-six").count() >= 2 || e.capture("g:").lines().filter(|l| *l == "marker-after-six").count() >= 1), "the picture script did not run");
    std::thread::sleep(Duration::from_millis(1500));
    c.send_str(&format!("sh {six}\r"));
    assert!(wait_until(LONG, || e.capture("g:").lines().filter(|l| *l == "marker-after-six").count() >= 2), "the second run did not finish");
    std::thread::sleep(Duration::from_millis(800));
    c.send(b"\x02d");
    c.wait_exit(Duration::from_secs(10));
    e.kill_server();
    std::thread::sleep(Duration::from_millis(500));
    c
}

#[test]
fn g_images() {
    let mut e = Env::new("gimg");
    let conf = p(&e.path("imux.conf"));
    e.set("IMUX_CONFIG", &conf);
    e.unset("SHELL");
    let sixel = base64_encode(b"\x1bPq\"1;1;2;6#0;2;100;0;0#0B?\x1b\\");
    e.write("six.sh", &format!("printf '\\033]1337;File=inline=1;width=1;height=1;preserveAspectRatio=0:%s\\007' '{sixel}'\necho marker-after-six\n"));
    let mut t = Check::new("g_images");
    let has = |c: &Client, s: &str| c.has_bytes(s.as_bytes());

    t.section("a Sixel stream sent as an iTerm2 file");
    let c = run_with(&e, &[("TERM_PROGRAM", "iTerm.app")], "");
    t.ok("an iterm2 client gets the picture as an inline image", has(&c, &format!("{ESC}]1337;File=")));
    t.ok("and the text after it still arrives", c.seen("marker-after-six"));

    t.section("a forced image-mode is tried first");
    let c = run_with(&e, &[("TERM_PROGRAM", "iTerm.app")], "set image-mode sixel");
    t.ok("image-mode sixel gives an iterm2 client Sixel", has(&c, &format!("{ESC}P0;1;0q")));
    t.ok("and no inline image", !has(&c, &format!("{ESC}]1337;File=")));
    let c = run_with(&e, &[("TERM_PROGRAM", "Apple_Terminal")], "set image-mode kitty");
    t.ok("image-mode kitty gives any client Kitty graphics", has(&c, &format!("{ESC}_Ga=t,i=1,")));
    t.ok("placed by id, a source rectangle", has(&c, &format!("{ESC}_Ga=p,i=1,p=1,")));
    t.ok("and no Sixel", !has(&c, &format!("{ESC}P0;1;0q")));
    let c = run_with(&e, &[("TERM", "foot")], "set image-mode iterm2");
    t.ok("image-mode iterm2 gives a Sixel terminal the inline image", has(&c, &format!("{ESC}]1337;File=")));
    t.ok("and no Sixel", !has(&c, &format!("{ESC}P0;1;0q")));

    t.section("without image-mode the order is iTerm2, Kitty, Sixel, blocks");
    let c = run_with(&e, &[("TERM", "xterm-kitty")], "set image-mode auto");
    t.ok("a Kitty terminal gets Kitty", has(&c, &format!("{ESC}_Ga=t,i=1,")));
    let c = run_with(&e, &[("TERM", "foot")], "");
    t.ok("a Sixel terminal gets Sixel", has(&c, &format!("{ESC}P0;1;0q")));
    let c = run_with(&e, &[("TERM_PROGRAM", "Apple_Terminal")], "");
    t.ok("Terminal.app gets no picture protocol", !has(&c, &format!("{ESC}P0;1;0q")));
    t.ok("not Kitty either", !has(&c, &format!("{ESC}_Ga=")));
    t.ok("not an inline image", !has(&c, &format!("{ESC}]1337;File=")));

    t.section("image-kitty-placement crop stores each crop");
    let c = run_with(&e, &[("TERM", "xterm-kitty")], "set image-kitty-placement crop");
    t.ok("crop mode still transmits and places", has(&c, &format!("{ESC}_Ga=t,i=1,")));

    t.section("image-glyphs");
    let quadrant = run_with(&e, &[("TERM_PROGRAM", "Apple_Terminal")], "");
    let sextant = run_with(&e, &[("TERM_PROGRAM", "Apple_Terminal")], "set image-glyphs sextant");
    let octant = run_with(&e, &[("TERM_PROGRAM", "Apple_Terminal")], "set image-glyphs octant");
    let sextant_lead: &[u8] = b"\xf0\x9f\xac";
    let octant_lead: &[u8] = b"\xf0\x9c";
    t.ok("quadrants draw the picture at all", has(&quadrant, "38;5;196m"));
    t.ok("quadrants use no sextant characters", !quadrant.has_bytes(sextant_lead));
    t.ok("quadrants use no octant characters", !quadrant.has_bytes(octant_lead));
    t.ok("sextants are drawn when asked for", sextant.has_bytes(sextant_lead));
    t.ok("octants are drawn when asked for", octant.has_bytes(octant_lead));
    t.finish();
}
