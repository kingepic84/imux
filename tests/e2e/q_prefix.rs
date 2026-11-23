//! `#{client_prefix}`: 1 while the client has pressed the prefix and not yet the key after it, in
//! the status line and in display-message; the bar redraws when the prefix is pressed and when it
//! is used. (The Rust version of tests/q_prefix.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn q_prefix() {
    let mut e = Env::new("qpf");
    let mut t = Check::new("q_prefix");
    let conf = e.write(
        "imux.conf",
        "set -g status-right \"[#{?client_prefix,PFXON,PFXOFF}]\"\nbind e display-message -p \"ONKEY=#{client_prefix}\"\n",
    );
    e.set("IMUX_CONFIG", &p(&conf));
    e.session("pfx", "start");
    let check = e.all(&["check-config", &p(&conf)]);
    t.ok("check-config knows client_prefix", !check.contains("client_prefix"));
    t.ok("no client, no prefix", e.out(&["display-message", "-p", "#{client_prefix}"]) == "0");
    let text = e.with_client("pfx", |c| {
        c.send(b"\x02");
        c.wait_seen("PFXON", Duration::from_secs(5));
        c.send(b"e");
        // the bar goes back to the prefix being off
        c.wait_screen("PFXOFF", Duration::from_secs(5));
    });
    t.ok("the bar starts with the prefix off", text.contains("PFXOFF"));
    t.ok("the bar redraws with the prefix on after C-b", text.contains("PFXON"));
    let plain = e.with_client("pfx", |c| e.barrier(c, "pfx"));
    t.ok("without C-b the bar never shows the prefix on", !plain.contains("PFXON"));
    e.run(&["new-window", "-d", "-n", "cmdw", "sleep 60"]);
    t.eq("pane_start_command is the command the pane was started with", e.out(&["display-message", "-p", "-t", "pfx:cmdw", "#{pane_start_command}"]), "sleep 60".to_string());
    t.ok("a plain shell pane has no start command", e.out(&["display-message", "-p", "-t", "pfx:start", "#{pane_start_command}"]).is_empty());
    e.kill_server();
    t.finish();
}
