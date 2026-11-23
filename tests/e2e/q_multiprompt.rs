//! command-prompt -p "a,b": prompts asked one after the other, answers as %1..%9 (and %% the last),
//! -I with a text per prompt, -T accepted, and a confirm-before label with a comma stays one label.
//! (The Rust version of tests/q_multiprompt.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn current(e: &Env) -> String {
    e.out(&["display-message", "-p", "#{window_name}"])
}

fn is(e: &Env, want: &str) -> bool {
    wait_until(LONG, || current(e) == want)
}

fn nap(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[test]
fn q_multiprompt() {
    let mut e = Env::new("qmp");
    let conf = e.write(
        "imux.conf",
        "bind a command-prompt -p \"first:,second:\" \"rename-window %1-%2\"\n\
         bind i command-prompt -I \"one,two\" -p \"x:,y:\" \"rename-window %2%1\"\n\
         bind l command-prompt -p \"a:,b:\" \"rename-window %%\"\n\
         bind t command-prompt -T command -p \"cmd:\" \"rename-window t%%\"\n\
         bind c confirm-before -p \"sure, really?\" \"rename-window sure\"\n\
         bind p command-prompt -p \"n:\" \"rename-window %%%1%2\"\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_multiprompt");
    e.session("mp", "start");

    let out = e.with_client("mp", |c| {
        c.send(b"\x02a");
        nap(500);
        c.send_str("AA\r");
        nap(500);
        c.send_str("BB\r");
        t.ok("two prompts, two answers: %1-%2", is(&e, "AA-BB"));
    });
    t.ok("the second label was shown", out.contains("second:"));

    e.with_client("mp", |c| {
        c.send(b"\x02i");
        nap(500);
        c.send(b"\r");
        nap(500);
        c.send(b"\r");
        t.ok("-I gives each prompt its own first text, %2%1", is(&e, "twoone"));
    });

    e.with_client("mp", |c| {
        c.send(b"\x02l");
        nap(500);
        c.send_str("x\r");
        nap(500);
        c.send_str("y\r");
        t.ok("%% is the last answer", is(&e, "y"));
    });

    e.with_client("mp", |c| {
        c.send(b"\x02a");
        nap(500);
        c.send_str("one\r");
        nap(500);
        c.send_str("half");
        nap(300);
        c.send(b"\x1b");
        nap(800);
    });
    t.ok("Escape at the second prompt cancels everything", current(&e) == "y");

    e.with_client("mp", |c| {
        c.send(b"\x02t");
        nap(500);
        c.send_str("ok\r");
        t.ok("-T is accepted", is(&e, "tok"));
    });

    e.with_client("mp", |c| {
        c.send(b"\x02c");
        nap(500);
        c.send(b"y");
        t.ok("a confirm label with a comma is one question", is(&e, "sure"));
    });

    e.with_client("mp", |c| {
        c.send(b"\x02p");
        nap(500);
        c.send_str("%2\r");
        t.ok("what is typed is not read for % again", is(&e, "%2%2"));
    });
    e.kill_server();
    t.finish();
}
