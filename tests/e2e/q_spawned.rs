//! P10.1, who may do what from a pane: `command-from-panes off|spawned|on`, the record of who made
//! a pane (`#{pane_spawned_by}`), a pane held to the panes it spawned, spawns that never take
//! focus, `spawned-depth` and `spawned-panes-max`, the commands that stay refused below `on`, the
//! prompt into agents, and the log line of each write. (The Rust version of tests/q_spawned.sh.)
use crate::support::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// Type a command into a pane as the user would; its status and output.
fn typed(e: &Env, pane: &str, command: &str) -> (i32, String) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let (o, rc) = (e.path(&format!("o{n}")), e.path(&format!("rc{n}")));
    e.run(&["send-keys", "-t", pane, "-l", &format!("{{ {command}; }} >{} 2>&1; echo $? >{}", o.display(), rc.display())]);
    e.run(&["send-keys", "-t", pane, "Enter"]);
    wait_until(Duration::from_secs(20), || std::fs::read_to_string(&rc).map(|s| !s.trim().is_empty()).unwrap_or(false));
    let code = std::fs::read_to_string(&rc).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(-1);
    (code, std::fs::read_to_string(&o).unwrap_or_default())
}

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

#[test]
fn q_spawned() {
    let mut e = Env::new("qsp");
    let mut t = Check::new("q_spawned");
    let conf = e.write("imux.conf", "set -g status off\nset -g agent-control on\n");
    e.set("IMUX_CONFIG", &p(&conf));
    e.set("IMUX_LOG", "info");
    let log = e.path("server.log");
    e.set("IMUX_LOG_FILE", &p(&log));
    e.session("sp", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    let b = BIN;
    let ty = |pane: &str, cmd: &str| typed(&e, pane, cmd);
    let active = || e.out(&["list-panes", "-F", "#{pane_id} #{pane_active}"]).lines().filter_map(|l| l.strip_suffix(" 1")).collect::<Vec<_>>().join(" ");
    let opt = |name: &str| e.out(&["show-options", "-gv", name]);

    t.section("the default");
    t.ok("a pane may act on the panes it spawned by default", opt("command-from-panes") == "spawned");
    let (rc, o) = ty("%0", &format!("{b} send-keys -t %0 x"));
    t.ok("and no more than that: its own pane is not one it spawned", rc != 0 && o.contains("command-from-panes spawned"));

    t.section("off: look only, as before");
    e.run(&["set", "-g", "command-from-panes", "off"]);
    let (rc, o) = ty("%0", &format!("{b} split-window -d -P -F '#{{pane_id}}'"));
    t.ok("a pane may not spawn while the option is off", rc != 0 && o.contains("command-from-panes on"));
    let (rc, o) = ty("%0", &format!("{b} list-panes -F '#{{pane_id}}'"));
    t.ok("it may look", rc == 0 && o.contains("%0"));

    t.section("spawned: a pane acts on the panes it spawned");
    e.run(&["set", "-g", "command-from-panes", "spawned"]);
    t.ok("the option shows the value", opt("command-from-panes") == "spawned");
    let (rc, o) = ty("%0", &format!("{b} split-window -P -F '#{{pane_id}}'"));
    t.ok("a pane can spawn a sibling", rc == 0 && has_line(&o, "%1"));
    t.ok("the spawn did not take the focus (no -d was typed)", active() == "%0");
    t.ok("the record says who made it", e.var("%1", "#{pane_spawned_by}") == "%0");
    t.ok("a pane the user made has no maker", e.var("%0", "#{pane_spawned_by}").is_empty());
    e.ready("%1", Duration::from_millis(400));
    let (rc, _) = ty("%0", &format!("{b} send-keys -t %1 'echo ZZHELLO-FROM-0' Enter"));
    t.ok("it can type into the pane it spawned", rc == 0);
    t.ok("and the text arrived", e.shows("%1", "^ZZHELLO-FROM-0"));
    let (rc, o) = ty("%0", &format!("{b} capture-pane -p -t %1"));
    t.ok("it can read the pane it spawned", rc == 0 && o.contains("ZZHELLO-FROM-0"));
    let (rc, o) = ty("%0", &format!("{b} send-keys -t %0 x"));
    t.ok("it may not type into its own pane", rc != 0 && o.contains("command-from-panes spawned"));
    let (rc, o) = ty("%0", &format!("{b} respawn-pane -t %1"));
    t.ok("a command that passes the check can still fail on its own", rc != 0 && o.contains("still running"));

    t.section("a pane the user made is not in the subtree");
    e.run(&["new-window", "-d"]);
    let user2 = e.out(&["list-panes", "-a", "-F", "#{pane_id}"]).lines().last().unwrap_or("").to_string();
    t.ok("the user made another pane", !user2.is_empty() && user2 != "%0" && user2 != "%1");
    let refused = "is not one this pane spawned";
    let (rc, o) = ty("%0", &format!("{b} send-keys -t {user2} x"));
    t.ok("no typing into it", rc != 0 && o.contains(refused));
    let (rc, o) = ty("%0", &format!("{b} kill-pane -t {user2}"));
    t.ok("no closing it", rc != 0 && o.contains(refused));
    let (rc, o) = ty("%0", &format!("{b} capture-pane -p -t {user2}"));
    t.ok("no reading it", rc != 0 && o.contains(refused));
    let (rc, o) = ty("%0", &format!("{b} split-window -t {user2} -P -F '#{{pane_id}}'"));
    t.ok("no splitting it", rc != 0 && o.contains(refused));
    let uw = e.var(&user2, "#{window_id}");
    let (rc, o) = ty("%0", &format!("{b} kill-window -t {uw}"));
    t.ok("no closing its window", rc != 0 && o.contains(refused));
    let (rc, _) = ty("%0", &format!("{b} send-keys -t '%1 %2' x"));
    t.ok("a target made of two ids is no target", rc != 0);
    let (rc, o) = ty("%0", &format!("{b} kill-pane -a -t %1"));
    t.ok("kill-pane -a is refused", rc != 0 && o.contains("command-from-panes spawned"));

    t.section("refused below on, whatever the target");
    for words in [
        "set-option -g @x 1",
        "set-environment -g X 1",
        "bind-key z kill-server",
        "set-hook -g after-new-window ls",
        "source-file /dev/null",
        "pipe-pane -t %1 cat",
        "run-shell true",
        "if-shell true true",
        "select-pane -t %1",
        "select-window -t :0",
        "kill-session -t sp",
        "new-session -d -s other",
        "display-menu -T t a b c",
        "rename-window -t %1 x",
        "resize-pane -t %1 -x 10",
    ] {
        let (rc, o) = ty("%0", &format!("{b} {words}"));
        t.ok(&format!("refused: {words}"), rc != 0 && !o.lines().any(|l| l.starts_with("ok")));
    }
    t.ok("no option was set", e.all(&["show-options", "-gv", "@x"]).contains("unknown option"));
    t.ok("no key was bound", !e.out(&["list-keys"]).contains("kill-server"));

    t.section("a pane that was spawned is held to its own subtree");
    e.ready("%1", Duration::from_millis(300));
    let (rc, o) = ty("%1", &format!("{b} split-window -P -F '#{{pane_id}}'"));
    t.matches("a spawned pane may spawn one level below", "^%[0-9]*$", o.lines().next().unwrap_or(""));
    t.ok("(it succeeded)", rc == 0);
    let grand = o.lines().next().unwrap_or("").to_string();
    t.ok("the focus is still where the user left it", active() == "%0");
    t.ok("the record chains", e.var(&grand, "#{pane_spawned_by}") == "%1");
    let (rc, o) = ty("%1", &format!("{b} send-keys -t %0 x"));
    t.ok("a child cannot type into its parent", rc != 0 && o.contains(refused));
    let (rc, o) = ty("%1", &format!("{b} kill-pane -t %0"));
    t.ok("nor close it", rc != 0 && o.contains(refused));
    let (rc, o) = ty("%1", &format!("{b} capture-pane -p -t %0"));
    t.ok("nor read it", rc != 0 && o.contains(refused));
    let (rc, _) = ty("%1", &format!("{b} set-option -g @y 1"));
    t.ok("nor set an option", rc != 0);
    e.ready(&grand, Duration::from_millis(300));
    let (rc, o) = ty(&grand, &format!("{b} split-window -P -F '#{{pane_id}}'"));
    t.ok("spawned-depth stops a third level", rc != 0 && o.contains("spawned-depth"));
    let (rc, _) = ty("%0", &format!("{b} send-keys -t {grand} 'echo ZZDEEP' Enter"));
    t.ok("the first pane reaches its grandchild", rc == 0);
    t.ok("and the text arrived", e.shows(&grand, "^ZZDEEP"));
    e.run(&["set", "-g", "spawned-depth", "3"]);
    let (rc, o) = ty(&grand, &format!("{b} new-window -P -F '#{{pane_id}}'"));
    t.ok("a larger spawned-depth allows a third level", rc == 0);
    let great = o.lines().next().unwrap_or("").to_string();
    t.ok("and the window did not become the current one", e.out(&["display-message", "-p", "#{window_index}"]) == "0");
    e.run(&["set", "-g", "spawned-depth", "2"]);
    let (rc, _) = ty("%0", &format!("{b} kill-pane -t {great}"));
    t.ok("the first pane closes what is below it", rc == 0);
    let (rc, _) = ty("%0", &format!("{b} kill-pane -t {grand}"));
    t.ok("and its grandchild", rc == 0);
    t.ok(
        "the pane is gone",
        wait_until(Duration::from_secs(5), || !e.out(&["list-panes", "-a", "-F", "#{pane_id}"]).lines().any(|l| l == grand)),
    );

    t.section("spawned-panes-max");
    e.run(&["set", "-g", "spawned-panes-max", "2"]);
    let (rc, o) = ty("%0", &format!("{b} new-window -P -F '#{{pane_id}}'"));
    t.ok("a window is a spawn too, and takes no focus", rc == 0 && e.out(&["display-message", "-p", "#{window_index}"]) == "0");
    let w2 = o.lines().next().unwrap_or("").to_string();
    let (rc, o) = ty("%0", &format!("{b} new-window -P -F '#{{pane_id}}'"));
    t.ok("past the count it is refused and names the option", rc != 0 && o.contains("spawned-panes-max"));
    let wid = e.var(&w2, "#{window_id}");
    let (rc, _) = ty("%0", &format!("{b} kill-window -t {wid}"));
    t.ok("kill-window works on a window of spawned panes", rc == 0);
    wait_until(Duration::from_secs(5), || !e.out(&["list-panes", "-a", "-F", "#{pane_id}"]).lines().any(|l| l == w2));
    let (rc, _) = ty("%0", &format!("{b} new-window -P -F '#{{pane_id}}'"));
    t.ok("closing one makes room", rc == 0);
    e.run(&["set", "-g", "spawned-panes-max", "16"]);

    t.section("agents");
    ty("%1", &format!("{b} pane report-agent --source t --agent fake --state idle"));
    ty(&user2, &format!("{b} pane report-agent --source t --agent fake --state idle"));
    let u2n = user2.trim_start_matches('%');
    t.ok(
        "both panes have an agent record",
        wait_until(Duration::from_secs(10), || e.out(&["agent", "get", "1"]).contains("fake") && e.out(&["agent", "get", u2n]).contains("fake")),
    );
    let (rc, _) = ty("%0", &format!("{b} agent prompt 1 hello"));
    t.ok("a prompt into a pane it spawned goes through", rc == 0);
    let (rc, o) = ty("%0", &format!("{b} agent prompt {u2n} hello"));
    t.ok("a prompt into a pane it did not spawn is refused", rc != 0 && o.contains(refused));
    e.run(&["set", "-g", "agent-control", "off"]);
    let (rc, o) = ty("%0", &format!("{b} agent prompt 1 hello"));
    t.ok("agent-control is still needed", rc != 0 && o.contains("agent-control"));
    e.run(&["set", "-g", "agent-control", "on"]);

    t.section("popups stay under their own option");
    e.run(&["set", "-g", "popup-from-panes", "off"]);
    let (rc, o) = ty("%0", &format!("{b} display-popup -E true"));
    t.ok("popup-from-panes off is refused and named", rc != 0 && o.contains("popup-from-panes"));
    e.run(&["set", "-g", "popup-from-panes", "on"]);

    t.section("on: a pane the user made can do what it could");
    e.run(&["set", "-g", "command-from-panes", "on"]);
    let (rc, _) = ty("%0", &format!("{b} set-option -g @fromzero 1"));
    t.ok("a user-made pane may configure", rc == 0 && opt("@fromzero") == "1");
    let (rc, _) = ty("%1", &format!("{b} set-option -g @fromone 1"));
    t.ok("a spawned pane is still held to its subtree", rc != 0 && e.all(&["show-options", "-gv", "@fromone"]).contains("unknown option"));
    let (rc, _) = ty("%1", &format!("{b} send-keys -t %0 x"));
    t.ok("and cannot reach its parent", rc != 0);

    t.section("back to off: a spawned pane is look-only");
    e.run(&["set", "-g", "command-from-panes", "off"]);
    let (rc, o) = ty("%1", &format!("{b} split-window -P -F '#{{pane_id}}'"));
    t.ok("nothing is spawned", rc != 0 && o.contains("command-from-panes on"));
    let (rc, o) = ty("%0", &format!("{b} send-keys -t %1 x"));
    t.ok("a user-made pane is look-only again", rc != 0 && o.contains("command-from-panes on"));

    t.section("doctor");
    let off = e.all(&["doctor"]);
    t.matches("imux doctor names the tier and what it means", "(?m)^pane commands: command-from-panes off: a pane may only look", &off);
    e.run(&["set", "-g", "command-from-panes", "spawned"]);
    let sp = e.all(&["doctor"]);
    t.matches(
        "and the spawn limits when the tier is spawned",
        "(?m)^pane commands: command-from-panes spawned: .*at most 2 levels deep and 16 live panes",
        &sp,
    );
    e.run(&["set", "-g", "command-from-panes", "off"]);

    t.section("the log");
    let l = e.read("server.log");
    t.ok("a write from a pane is logged with who, what and how it went", l.contains("pane-write caller=%0 power=subtree command=split-window"));
    t.matches("the target is in the line", "command=send-keys target=.*%1.* result=ok", &l);
    t.ok("a refusal is logged", l.contains("refused set-option from pane 0"));
    t.ok("a failed write says so", l.contains("result=failed"));
    e.kill_server();
    t.finish();
}
