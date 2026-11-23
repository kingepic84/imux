//! The gaps that group C closed: the terminal query (image-probe), text for pictures in copies,
//! nested programs in a snapshot, joined emoji (emoji-width), DCS replies, and the Kitty extras
//! (delete targets, placement ids, media files, unicode placeholders).
//! (The Rust version of tests/c_features.sh.)
use crate::support::*;
use std::path::Path;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);
const ESC: &str = "\x1b";

fn probe_client(e: &Env, term: &str) -> Client {
    let mut c = e.client_env(100, 30, &["new", "-s", "p"], &[("TERM_PROGRAM", term)], &["LC_TERMINAL", "COLORTERM", "KITTY_WINDOW_ID"]);
    assert!(e.has_client());
    assert!(e.ready("p:", Duration::from_millis(600)));
    c.send_str("echo up-$((1+2))\r");
    assert!(c.wait_for("up-3", LONG));
    std::thread::sleep(Duration::from_millis(500));
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    std::thread::sleep(Duration::from_millis(600));
    c
}

fn wait_file(path: &Path) -> bool {
    wait_until(LONG, || path.exists())
}

fn reply_of(e: &Env, text: &str, count: usize) -> Vec<u8> {
    let bin = e.path("reply.bin");
    let done = e.path("reply.bin.done");
    let _ = std::fs::remove_file(&bin);
    let _ = std::fs::remove_file(&done);
    e.run(&["send-keys", "-t", "c:0", &format!("clear; sh {} {} {count} '{text}'", p(&e.path("probe.sh")), bin.display()), "Enter"]);
    wait_file(&done);
    std::fs::read(&bin).unwrap_or_default()
}

#[test]
fn c_features() {
    let mut e = Env::new("cft");
    let conf = p(&e.path("imux.conf"));
    e.set("IMUX_CONFIG", &conf);
    e.unset("SHELL");
    let mut t = Check::new("c_features");

    t.section("image-probe");
    e.write("imux.conf", "");
    let apple = probe_client(&e, "Apple_Terminal");
    t.ok("Terminal.app is not asked by default", !apple.has_bytes(format!("{ESC}_Gi=31").as_bytes()));
    t.ok("and the session works", apple.seen("up-3"));
    e.write("imux.conf", "set image-probe always\n");
    let always = probe_client(&e, "Apple_Terminal");
    t.ok("image-probe always asks it anyway", always.has_bytes(format!("{ESC}_Gi=31").as_bytes()));
    e.write("imux.conf", "set image-probe never\n");
    let never = probe_client(&e, "iTerm.app");
    t.ok("image-probe never asks nobody", !never.has_bytes(format!("{ESC}_Gi=31").as_bytes()));
    e.write("imux.conf", "");

    let pixfile = p(&e.write("pixel.rgb", ""));
    std::fs::write(&pixfile, b"\xff\x00\x00").unwrap();
    e.set("PIXFILE", &pixfile);
    e.write(
        "pic.sh",
        r#"printf 'A '
printf '\033_Ga=T,f=24,s=1,v=1,c=3,r=1,i=1,q=2;/wAA\033\\'
printf 'B '
printf '\033_Ga=T,f=24,s=1,v=1,c=3,r=1,i=2,q=2;/wAA\033\\'
printf '\033_Ga=d,d=i,i=2\033\\'
printf 'C '
printf '\033_Ga=T,U=1,f=24,s=1,v=1,c=2,r=1,i=7,q=2;/wAA\033\\'
printf '\033[38;5;7m\364\216\273\256\314\205\314\205\364\216\273\256\033[0m\n'
printf 'D '
printf '\033_Ga=T,t=f,f=24,s=1,v=1,i=9,q=2;%s\033\\\n' "$(printf '%s' "$PIXFILE" | base64)"
touch "$1"
"#,
    );
    e.session("c", "w");
    assert!(e.ready("c:0", Duration::from_millis(600)));
    let pic = p(&e.path("pic.sh"));
    let pdone = e.path("pic.done");
    e.run(&["send-keys", "-t", "c:0", &format!("PIXFILE={pixfile} sh {pic} {}", pdone.display()), "Enter"]);
    assert!(wait_file(&pdone), "the picture script did not finish");
    std::thread::sleep(Duration::from_secs(1));

    t.section("pictures in copied text, delete targets, placeholders, media");
    let cap1 = e.capture("c:0");
    t.ok("a picture is [image] in capture-pane", cap1.contains("A [image]"));
    t.ok("a deleted picture (d=i) leaves nothing", !cap1.contains("B [image]"));
    t.ok("the row of a deleted picture still has its text", cap1.contains('B'));
    t.ok("placeholder characters show the picture once per run", cap1.contains("C [image]"));
    t.ok("a placeholder character does not show as text", !cap1.contains('\u{10eede}') && !cap1.chars().any(|c| ('\u{100000}'..='\u{10ffff}').contains(&c)));
    t.ok("a file is not read while kitty-media is none", !cap1.contains("D [image]"));
    e.run(&["set", "-g", "image-copy-text", "<pic>"]);
    t.ok("image-copy-text changes the text", e.capture("c:0").contains("A <pic>"));
    e.run(&["set", "-g", "image-copy-text", ""]);
    t.ok("and an empty one leaves the picture out", !e.capture("c:0").contains("[image]"));
    e.run(&["set", "-g", "image-copy-text", "[image]"]);
    e.run(&["set", "-g", "kitty-media", "files"]);
    let _ = std::fs::remove_file(&pdone);
    e.run(&["send-keys", "-t", "c:0", &format!("clear; PIXFILE={pixfile} sh {pic} {}", pdone.display()), "Enter"]);
    assert!(wait_file(&pdone));
    std::thread::sleep(Duration::from_secs(1));
    t.ok("with kitty-media files the file picture is shown", e.capture("c:0").contains("D [image]"));
    e.run(&["set", "-g", "kitty-media", "none"]);

    e.write("probe.sh", "stty raw -echo\nprintf \"$3\"\ndd bs=1 count=\"$2\" of=\"$1\" 2>/dev/null\nstty sane\ntouch \"$1.done\"\n");
    t.section("DCS replies");
    t.ok("XTGETTCAP TN is answered with a fixed value", reply_of(&e, "\\033P+q544e\\033\\\\", 20) == b"\x1bP1+r544E=696D7578\x1b\\");
    t.ok("an unknown setting request is an invalid request without an echo", reply_of(&e, "\\033P$qsecret\\033\\\\", 7) == b"\x1bP0$r\x1b\\");

    t.section("emoji-width");
    let family = "\\360\\237\\221\\250\\342\\200\\215\\360\\237\\221\\251\\342\\200\\215\\360\\237\\221\\247";
    t.ok("by default the three people take six cells (column 7)", reply_of(&e, &format!("{family}\\033[6n"), 6) == b"\x1b[1;7R");
    e.run(&["set", "-g", "emoji-width", "cluster"]);
    t.ok("with emoji-width cluster they are one glyph of two cells (column 3)", reply_of(&e, &format!("{family}\\033[6n"), 6) == b"\x1b[1;3R");
    e.run(&["set", "-g", "emoji-width", "parts"]);

    t.section("a program inside a nested shell");
    e.kill_server();
    std::thread::sleep(Duration::from_secs(1));
    e.write("imux.conf", "set restore-commands \"sleep\"\n");
    e.session("n", "w");
    assert!(e.ready("n:0", Duration::from_millis(600)));
    e.run(&["send-keys", "-t", "n:0", "sh", "Enter"]);
    std::thread::sleep(Duration::from_secs(1));
    e.run(&["send-keys", "-t", "n:0", "sleep 317", "Enter"]);
    assert!(wait_until(LONG, || pgrep("sleep 317")));
    std::thread::sleep(Duration::from_millis(800));
    e.run(&["save", "nested"]);
    let snaps = e.path("state/snapshots");
    let recorded = std::fs::read_dir(&snaps).map(|d| d.flatten().any(|f| std::fs::read_to_string(f.path()).unwrap_or_default().contains("+outer 1 1 sh"))).unwrap_or(false);
    t.ok("the snapshot records the shell the program runs in", recorded);
    e.kill_server();
    std::thread::sleep(Duration::from_millis(1500));
    e.session("other", "w");
    e.run(&["restore", "nested"]);
    t.ok("restore types the shell and then the program", wait_until(LONG, || e.out(&["list-panes", "-a", "-F", "#{pane_current_command}"]).lines().any(|l| l == "sleep")));
    e.kill_server();
    pkill("sleep 317");
    t.finish();
}
