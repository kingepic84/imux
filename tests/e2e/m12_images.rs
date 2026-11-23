//! A picture sent as an iTerm2 inline image reaches each client in the form its terminal takes:
//! Kitty graphics, Sixel, an inline image, or 256-color blocks, and every client asks the terminal
//! what it supports. (The Rust version of tests/m12_images.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);
const ESC: &str = "\x1b";
const PNG: &str = "R0lGODlhAQABAIAAAP8AAAAAACwAAAAAAQABAAACAkQBADs=";

fn run_client(e: &Env, set: &[(&str, &str)]) -> Client {
    let mut c = e.client_env(100, 30, &["new", "-s", "img"], set, &["LC_TERMINAL", "COLORTERM", "TERM_PROGRAM", "KITTY_WINDOW_ID"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(e.ready("img:", Duration::from_millis(600)));
    let script = p(&e.path("img.sh"));
    c.send_str(&format!("sh {script}\r"));
    assert!(wait_until(LONG, || e.capture("img:").lines().any(|l| l == "marker-after-image")), "the picture script did not run");
    e.barrier(&c, "img");
    c.send_str(&format!("sh {script}\r"));
    assert!(wait_until(LONG, || e.capture("img:").lines().filter(|l| *l == "marker-after-image").count() >= 2));
    e.barrier(&c, "img");
    c.send(b"\x02d");
    c.wait_exit(Duration::from_secs(10));
    e.kill_server();
    c
}

#[test]
fn m12_images() {
    let e = Env::new("m12");
    e.write("img.sh", &format!("printf '\\033]1337;File=inline=1;width=4;height=2;preserveAspectRatio=0:%s\\007' '{PNG}'\necho marker-after-image\n"));
    let probe = e.write("probe.conf", "set image-probe always\n");
    let mut t = Check::new("m12_images");
    let has = |c: &Client, s: &str| c.has_bytes(s.as_bytes());

    let iterm = run_client(&e, &[("TERM_PROGRAM", "iTerm.app")]);
    let apple = run_client(&e, &[("TERM_PROGRAM", "Apple_Terminal"), ("IMUX_CONFIG", &p(&probe))]);
    let kitty = run_client(&e, &[("TERM", "xterm-kitty")]);
    let sixel = run_client(&e, &[("TERM", "foot")]);

    t.ok("kitty client gets a graphics command", has(&kitty, &format!("{ESC}_Ga=t,i=1,f=100,t=d,q=2")));
    t.ok("kitty client places the stored picture by id", has(&kitty, &format!("{ESC}_Ga=p,i=1,p=1,")));
    t.ok("kitty client gets no iterm2 image", !has(&kitty, &format!("{ESC}]1337;File=")));
    t.ok("sixel client gets a sixel image", has(&sixel, &format!("{ESC}P0;1;0q")));
    t.ok("sixel client gets no kitty command", !has(&sixel, &format!("{ESC}_Ga=")));
    t.ok("every client asks the terminal what it supports", has(&apple, &format!("{ESC}_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA{ESC}\\{ESC}[c")));
    t.ok("iterm2 client gets an inline image", has(&iterm, &format!("{ESC}]1337;File=inline=1;width=4;height=2;preserveAspectRatio=0:")));
    t.ok("iterm2 client: text after the image still arrives", iterm.seen("marker-after-image"));
    t.ok("terminal.app client gets no inline image", !has(&apple, &format!("{ESC}]1337;File=")));
    t.ok("terminal.app client: text after the image still arrives", apple.seen("marker-after-image"));
    t.ok("terminal.app client gets 256-color backgrounds for the picture", has(&apple, "48;5;"));
    t.ok("terminal.app client gets no truecolor", !has(&apple, "38;2;"));
    t.finish();
}
