//! The links of spawned panes in a snapshot are data from a file: ones that point nowhere, at the
//! pane itself, round a circle, or that are not numbers are dropped, and a restore still works.
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

#[test]
fn s_spawnlinks() {
    let mut e = Env::new("ssl");
    let conf = e.write("imux.conf", "set -g status off\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("s_spawnlinks");
    e.session("sp", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    hold(&e, "0");
    let (rc, o) = in_pane(&e, "0", &format!("{BIN} new-window -d -P -F '#{{pane_id}}' -t sp"));
    t.ok("a pane spawns one", rc == 0 && o.trim().starts_with('%'));
    t.ok("saved", e.run(&["save", "hostile"]).code == 0);
    let path = e.path("state/snapshots/hostile.imux");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let line = text.lines().find(|l| l.starts_with("meta imux.spawn ")).unwrap_or("").to_string();
    t.ok("the snapshot has the links", !line.is_empty());
    // one window points at the pane itself and at a pane in a window that does not exist, at a
    // circle (the first pane made by the second, which the file also has as made by the first), at
    // words, and at a number that is far too big
    let hostile = "meta imux.spawn 0:0.1.0,0:0.0.0,0:9.9.9,x:y,0:0.0,99999999999999999999:0.0.0,0:0.0.0.0";
    std::fs::write(&path, text.replace(&line, hostile)).unwrap();
    e.kill_server();

    let mut c = e.client(100, 30, &["new", "-s", "fresh"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    let r = e.all(&["restore", "hostile"]);
    t.matches("restore works", "restored 1 session", &r);
    let panes = e.out(&["list-panes", "-a", "-F", "#{pane_id}"]);
    let makers: Vec<(String, String)> = panes.lines().map(|id| (id.to_string(), e.var(id, "#{pane_spawned_by}"))).collect();
    t.ok("no pane is its own maker", makers.iter().all(|(id, by)| id != by));
    t.ok("the server is still there", e.run(&["list-panes", "-a"]).code == 0);
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    t.finish();
}
