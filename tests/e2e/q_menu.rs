//! `display-menu`: a binding that opens a menu, choosing by shortcut, by the movement keys and Enter,
//! by a click, closing by Escape and by a click outside, a disabled item, what the box shows, the
//! parse errors, and the refusal from inside a pane. (The Rust version of tests/q_menu.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn name(e: &Env) -> String {
    e.out(&["list-windows", "-F", "#{window_name}"]).lines().next().unwrap_or("").to_string()
}

fn name_is(e: &Env, want: &str) -> bool {
    wait_until(LONG, || name(e) == want)
}

#[test]
fn q_menu() {
    let mut e = Env::new("qmn");
    let conf = e.write(
        "imux.conf",
        "bind m display-menu -T \"Pick one\" \"Alpha\" a \"rename-window alpha\" \"\" \"Beta\" b \"rename-window beta\" \"-Gone\" g \"rename-window gone\"\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_menu");

    e.session("mnu", "start");
    t.ok("the binding with a menu loaded", e.all(&["list-keys"]).contains("display-menu"));

    let out = e.with_client("mnu", |c| {
        c.send(b"\x02m");
        // the box is drawn before a key closes it
        c.wait_seen("(b)", LONG);
        c.send(b"a");
        t.ok("a shortcut chooses its item", name_is(&e, "alpha"));
    });
    t.ok("the box shows the title, the names and the keys", out.contains("Pick one") && out.contains("Beta") && out.contains("(b)"));

    e.with_client("mnu", |c| {
        c.send(b"\x02m");
        c.send(b"j");
        c.send(b"\r");
        t.ok("the movement keys skip the rule and Enter chooses", name_is(&e, "beta"));
    });
    e.run(&["rename-window", "keep"]);

    e.with_client("mnu", |c| {
        c.send(b"\x02m");
        c.send(b"g");
        c.send(b"\x1b");
        c.escape_pause();
        c.send(b"\x02m");
        c.send(b"a");
        t.ok("a disabled item does not run, Escape closes the menu, and a new one opens after it", name_is(&e, "alpha"));
    });
    e.run(&["rename-window", "keep"]);

    e.with_client("mnu", |c| {
        c.send(b"\x02m");
        c.send(b"q");
        c.send(b"x");
        c.sync();
    });
    t.ok("q closes it and nothing ran", name(&e) == "keep");

    e.with_client("mnu", |c| {
        c.send(b"\x02m");
        c.send(b"\x1b[<0;47;15M\x1b[<0;47;15m");
        t.ok("a click on an item chooses it", name_is(&e, "beta"));
    });
    e.run(&["rename-window", "keep"]);

    e.with_client("mnu", |c| {
        c.send(b"\x02m");
        c.send(b"\x1b[<0;2;2M\x1b[<0;2;2m");
        c.send(b"a");
        c.sync();
    });
    t.ok("a click outside closes it and the key after it is not a shortcut", name(&e) == "keep");

    t.ok("an item without its key and command is an error", e.all(&["display-menu", "-T", "x", "a"]).contains("not enough arguments"));
    t.ok("an unknown border is an error", e.all(&["display-menu", "-b", "wavy", "a", "b", "c"]).contains("display-menu: -b is one of"));
    t.ok("a bad style is an error", e.all(&["display-menu", "-s", "nonsense", "a", "b", "c"]).contains("display-menu: -s"));
    t.ok("an unknown flag is an error", e.all(&["display-menu", "-z", "a", "b", "c"]).contains("unknown flag -z"));
    t.ok("outside a pane with no client attached it says so", e.all(&["display-menu", "a", "b", "c"]).contains("no client is attached"));
    e.run(&["send-keys", "-t", "%0", "C-u"]);
    e.run(&["send-keys", "-t", "%0", "-l", &format!("{BIN} display-menu a b c 2>&1 | grep -c 'not allowed' | sed 's/^/COUNT:/'")]);
    e.run(&["send-keys", "-t", "%0", "Enter"]);
    let re = regex_lite::Regex::new("COUNT:[1-9]").unwrap();
    t.ok("from inside a pane it is refused", wait_until(LONG, || re.is_match(&e.capture("%0"))));
    e.kill_server();
    t.finish();
}
