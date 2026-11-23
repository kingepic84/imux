//! Mouse keys in `bind -n`: a press, the wheel, a drag, a second press, a modifier, the border, the
//! status line, the variables a binding sees (#{mouse_x}, #{mouse_y}, #{mouse_word}), a menu opened
//! at the pointer, an event with no binding going on to what imux does by itself, and the names in
//! `list-keys` and the errors. (The Rust version of tests/q_mouse.sh.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn nap(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn log_has(e: &Env, line: &str) -> bool {
    wait_until(LONG, || e.read("log").lines().any(|l| l == line))
}

fn pane_width(e: &Env) -> usize {
    e.out(&["list-panes", "-F", "#{pane_width}"]).lines().next().unwrap_or("0").parse().unwrap_or(0)
}

#[test]
fn q_mouse() {
    let mut e = Env::new("qmo");
    let log = p(&e.path("log"));
    e.write("log", "");
    let conf = e.write(
        "imux.conf",
        &format!(
            "set -g status-right \"\"\n\
             bind -n MouseDown3Pane run-shell \"echo DOWN3:#{{mouse_x}}:#{{mouse_y}}:#{{mouse_any_flag}} >> {log}\"\n\
             bind -n WheelUpPane run-shell \"echo WHEELUP >> {log}\"\n\
             bind -n WheelDownStatusDefault run-shell \"echo WHEELSTATUS >> {log}\"\n\
             bind -n MouseDown3StatusLeft run-shell \"echo STATUSLEFT >> {log}\"\n\
             bind -n MouseDown3StatusDefault run-shell \"echo STATUSDEFAULT >> {log}\"\n\
             bind -n MouseDrag1Pane run-shell \"echo DRAG >> {log}\"\n\
             bind -n MouseDragEnd1Pane run-shell \"echo DRAGEND >> {log}\"\n\
             bind -n DoubleClick1Pane run-shell \"echo DOUBLE:#{{q:mouse_word}} >> {log}\"\n\
             bind -n C-MouseDown1Pane run-shell \"echo CTRLDOWN >> {log}\"\n\
             bind -n MouseDown3Border run-shell \"echo BORDER >> {log}\"\n\
             bind -n MouseDown2Pane display-menu -x M -y M -T Mm \"Alpha\" a \"run-shell 'echo MENUA >> {log}'\"\n"
        ),
    );
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_mouse");
    e.session("mo", "win");
    e.run(&["split-window", "-h"]);
    e.run(&["select-pane", "-t", "%0"]);
    assert!(e.ready("%0", Duration::from_millis(600)));
    let keys = e.all(&["list-keys"]);
    t.ok("list-keys shows the mouse keys by name", keys.contains("MouseDown3Pane") && keys.contains("C-MouseDown1Pane") && keys.contains("WheelDownStatusDefault"));
    e.run(&["send-keys", "-t", "%0", "-l", "echo MARKERWORD"]);
    e.run(&["send-keys", "-t", "%0", "Enter"]);
    assert!(e.shows_line("%0", "MARKERWORD"));

    e.with_client("mo", |c| {
        let w0 = pane_width(&e);
        for seq in [
            "\x1b[<2;5;5M\x1b[<2;5;5m".to_string(),
            "\x1b[<64;5;5M".to_string(),
            "\x1b[<65;90;30M".to_string(),
            "\x1b[<2;2;30M\x1b[<2;2;30m".to_string(),
            "\x1b[<2;90;30M\x1b[<2;90;30m".to_string(),
            "\x1b[<16;5;5M\x1b[<16;5;5m".to_string(),
            format!("\x1b[<2;{0};5M\x1b[<2;{0};5m", w0 + 1),
        ] {
            c.send_str(&seq);
            nap(400);
        }
        t.ok("a press with the right button, with the pointer's place in the pane and the program's mouse flag", log_has(&e, "DOWN3:4:4:0"));
        t.ok("the wheel over a pane", log_has(&e, "WHEELUP"));
        t.ok("the wheel over the empty part of the status line", log_has(&e, "WHEELSTATUS"));
        t.ok("a press on the session part of the status line", log_has(&e, "STATUSLEFT"));
        t.ok("a press on the empty part of the status line", log_has(&e, "STATUSDEFAULT"));
        t.ok("a modifier is part of the key", log_has(&e, "CTRLDOWN"));
        t.ok("a press on the line between two panes", log_has(&e, "BORDER"));
    });

    e.with_client("mo", |c| {
        c.send_str("\x1b[<0;5;5M");
        nap(300);
        c.send_str("\x1b[<32;8;5M");
        nap(300);
        c.send_str("\x1b[<0;8;5m");
        t.ok("a drag and the end of it, by the place the drag began", log_has(&e, "DRAG") && log_has(&e, "DRAGEND"));
    });

    e.with_client("mo", |c| {
        let row = e.capture("%0").lines().position(|l| l == "MARKERWORD").map(|i| i + 1).unwrap_or(1);
        c.send_str(&format!("\x1b[<0;3;{row}M\x1b[<0;3;{row}m\x1b[<0;3;{row}M\x1b[<0;3;{row}m"));
        t.ok("a second press on the same cell, with the word under the pointer", log_has(&e, "DOUBLE:MARKERWORD"));
    });

    e.run(&["select-pane", "-t", "%0"]);
    e.with_client("mo", |c| {
        let w0 = pane_width(&e);
        c.send_str(&format!("\x1b[<0;{0};5M\x1b[<0;{0};5m", w0 + 5));
        t.ok("a press that has no binding goes on to the focus change", wait_until(LONG, || e.out(&["list-panes", "-F", "#{pane_id}:#{pane_active}"]).lines().any(|l| l == "%1:1")));
    });

    let out = e.with_client("mo", |c| {
        c.send_str("\x1b[<1;30;10M\x1b[<1;30;10m");
        nap(800);
        c.send(b"a");
        t.ok("a menu opened by a mouse key takes its keys, placed at the pointer", log_has(&e, "MENUA"));
    });
    t.ok("the menu showed its title", out.contains("Mm"));

    let bad = e.write("bad.conf", "bind -n MouseDown4Pane display-message x\nbind -n MouseDown1 display-message x\n");
    t.ok("a mouse key name needs a button from 1 to 3 and a place", e.all(&["source-file", &p(&bad)]).contains("unknown key: MouseDown4Pane"));
    e.kill_server();
    t.finish();
}
