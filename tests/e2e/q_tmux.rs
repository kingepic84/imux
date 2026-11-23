//! `tmux-compat`: a pane gets $TMUX, $TMUX_PANE and a `tmux` first on PATH that runs imux's commands
//! with tmux's command line; the calls fzf-tab and `fzf --tmux` make (display-message with the
//! pane's geometry and option names as variables, list-commands, display-popup with attached flag
//! values and the exit status of its program); and `tmux-compat off`.
//! (The Rust version of tests/q_tmux.sh.)
use crate::support::*;
use regex_lite::Regex;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(10);

fn say(e: &Env, text: &str, pane: &str) {
    e.run(&["send-keys", "-t", pane, "C-u"]);
    e.run(&["send-keys", "-t", pane, "-l", text]);
    e.run(&["send-keys", "-t", pane, "Enter"]);
}

fn shows(e: &Env, pane: &str, pattern: &str) -> bool {
    Regex::new(&translate_pattern(pattern)).unwrap().is_match(&e.capture(pane))
}

fn wait_shows(e: &Env, pane: &str, pattern: &str) -> bool {
    wait_until(LONG, || shows(e, pane, pattern))
}

#[test]
fn q_tmux() {
    let mut e = Env::new("qtm");
    e.set("SHELL", "/bin/bash");
    let d = p(&e.dir);
    let mut t = Check::new("q_tmux");
    e.session("tm", "w");
    assert!(e.ready("%0", Duration::from_millis(600)));

    // what is typed to one shell runs in the order it was typed: no waiting between the lines
    say(&e, "printf \"BEFORE:%s\\n\" \"$(command -v tmux)\"", "%0");
    say(&e, &format!("eval \"$({BIN} init bash)\""), "%0");
    say(&e, "printf \"ENV:%s:%s\\n\" \"$TMUX_PANE\" \"$(command -v tmux)\"", "%0");
    t.ok("a pane has TMUX_PANE", wait_shows(&e, "%0", "ENV:%0:"));
    t.ok("and a tmux that is the shim", shows(&e, "%0", "/shims/tmux"));
    say(&e, "case \"$TMUX\" in */default,[0-9]*,0) echo TMUXOK;; *) echo \"TMUXBAD:$TMUX\";; esac", "%0");
    t.ok("TMUX is socket, pid and session", wait_shows(&e, "%0", "TMUXOK"));
    say(&e, "tmux -V | sed \"s/^/VER:/\"", "%0");
    t.ok("tmux -V prints a tmux version line", wait_shows(&e, "%0", "VER:tmux 3\\.[0-9]*$"));
    say(&e, "tmux display-message -p \"#{pane_top} #{cursor_y} #{pane_left} #{cursor_x} #{window_height} #{window_width} #{status} #{status-position}\" | sed \"s/^/FT:/\"", "%0");
    t.ok("display-message gives the geometry, the cursor and option names as variables", wait_shows(&e, "%0", "FT:0 [0-9]* 0 [0-9]* [0-9]* [0-9]* on bottom$"));
    say(&e, "tmux display-message -p -t \"$TMUX_PANE\" \"#{window_width} #{window_height}\" \\; list-commands new-pane 2>/dev/null; echo RC:$?", "%0");
    t.ok("the check fzf makes sees no new-pane command", wait_shows(&e, "%0", "RC:1") && !shows(&e, "%0", "^new-pane"));
    say(&e, "tmux list-commands display-popup", "%0");
    t.ok("list-commands names a command it has", wait_shows(&e, "%0", "^display-popup$"));
    say(&e, "tmux no-such-thing 2>&1 | sed \"s/^/ERR:/\"", "%0");
    t.ok("an unknown command is an error on stderr", wait_shows(&e, "%0", "ERR:"));

    e.with_client("tm", |_c| {
        say(&e, "for c in 0 1 130; do tmux popup -E sh -c \"exit $c\"; echo PS$c:$?; done", "%0");
        wait_shows(&e, "%0", "PS130:130");
    });
    t.ok("display-popup ends with the status of its program (0)", shows(&e, "%0", "PS0:0"));
    t.ok("display-popup ends with the status of its program (1)", shows(&e, "%0", "PS1:1"));
    t.ok("display-popup ends with the status of its program (130)", shows(&e, "%0", "PS130:130"));

    e.write("ftb", &format!("stty size > {d}/sz\necho FTB > {d}/res\n"));
    e.with_client("tm", |_c| {
        say(&e, &format!("tmux popup -x 5 -y 12 -w 30 -h 8 -d $PWD -E \". {d}/ftb\"; echo FTBDONE:$?"), "%0");
        wait_shows(&e, "%0", "FTBDONE:0");
    });
    t.ok("a popup with attached or separate flag values runs a sourced file and returns", shows(&e, "%0", "FTBDONE:0") && e.read("res").contains("FTB"));
    t.ok("its screen is the box inside the border", e.read("sz").trim() == "6 28");
    e.with_client("tm", |_c| {
        say(&e, &format!("tmux display-popup -E -d $PWD -xC -yC -w80% -h50% sh {d}/ftb; echo FZFDONE:$?"), "%0");
        wait_shows(&e, "%0", "FZFDONE:0");
    });
    t.ok("the shape of the fzf call works: attached values and a program with arguments", shows(&e, "%0", "FZFDONE:0"));

    if which("fzf").is_some() {
        e.with_client("tm", |_c| {
            say(&e, &format!("printf 'alpha\\nbeta\\n' | fzf --tmux -q alp --select-1 > {d}/fzfout; echo REALFZF:$?"), "%0");
            wait_shows(&e, "%0", "REALFZF:0");
        });
        t.ok("fzf --tmux opens a popup through the shim and returns its choice", shows(&e, "%0", "REALFZF:0") && e.read("fzfout").trim() == "alpha" && e.server_log().contains("opened by pane"));
    } else {
        println!("skip fzf --tmux (fzf is not installed)");
    }
    let ftblib = "/opt/homebrew/opt/fzf-tab/share/fzf-tab/lib";
    if which("fzf").is_some() && which("zsh").is_some() && std::path::Path::new(&format!("{ftblib}/ftb-tmux-popup")).exists() {
        e.write("ftbin", "alpha\nbeta\ngamma\n");
        e.write("ftb.zsh", &format!("zmodload zsh/mapfile\nfpath=({ftblib} $fpath)\nautoload -Uz ftb-tmux-popup\n_ftb_curcontext=x\nftb-tmux-popup -q alp --select-1 < {d}/ftbin > {d}/ftbout\necho RET:$? > {d}/ftbret\n"));
        e.with_client("tm", |_c| {
            say(&e, &format!("zsh -f {d}/ftb.zsh; echo FTBTAB"), "%0");
            wait_shows(&e, "%0", "FTBTAB");
        });
        t.ok("fzf-tab's own ftb-tmux-popup places and runs its popup and returns the choice", e.read("ftbout").trim() == "alpha" && e.read("ftbret").trim() == "RET:0");
    } else {
        println!("skip fzf-tab (fzf, zsh or fzf-tab is not installed)");
    }

    e.run(&["set", "-g", "tmux-compat", "off"]);
    e.run(&["new-window", "-d", "-n", "off"]);
    let offp = e.out(&["list-panes", "-t", "tm:off", "-F", "#{pane_id}"]).lines().next().unwrap_or("").to_string();
    assert!(e.ready(&offp, Duration::from_millis(600)));
    say(&e, "echo \"OFF:${TMUX_PANE:-none}:${TMUX:-none}\"", &offp);
    t.ok("with tmux-compat off a new pane has none of it", wait_shows(&e, &offp, "OFF:none:none"));
    e.kill_server();
    t.finish();
}
