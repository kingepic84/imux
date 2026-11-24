//! A program in a pane keeps the panes it spawned through a save and a restore: the snapshot holds
//! who made which pane (by position), and the restored panes, which have new numbers, get the same
//! makers, so `command-from-panes spawned` still lets the program act on them (and still not on
//! anything else). (Found on a real run: after a restore the agent could not manage its panes.)
use crate::support::*;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(20);

#[test]
fn q_spawnrestore() {
    let mut e = Env::new("qsr");
    let conf = e.write("imux.conf", "set -g status off\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_spawnrestore");
    let b = BIN;

    t.section("a program spawns a pane, which spawns another");
    e.session("sp", "w");
    assert!(e.ready("%0", Duration::from_millis(500)));
    hold(&e, "0");
    let (rc, o) = in_pane(&e, "0", &format!("{b} new-window -d -P -F '#{{pane_id}}' -t sp"));
    let child = o.trim().to_string();
    t.ok("the first pane spawns one", rc == 0 && child.starts_with('%'));
    let cn = child.trim_start_matches('%').to_string();
    assert!(e.ready(&child, Duration::from_millis(500)));
    hold(&e, &cn);
    let (rc, o) = in_pane(&e, &cn, &format!("{b} new-window -d -P -F '#{{pane_id}}' -t sp"));
    let grand = o.trim().to_string();
    t.ok("which spawns another", rc == 0 && grand.starts_with('%'));
    let user = e.out(&["new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "sp"]);
    t.ok("and the user makes one of their own", user.starts_with('%'));
    t.ok("the records say so", e.var(&child, "#{pane_spawned_by}") == "%0" && e.var(&grand, "#{pane_spawned_by}") == child && e.var(&user, "#{pane_spawned_by}").is_empty());
    let r = e.run(&["save", "spawnsnap"]);
    t.ok("saved", r.code == 0);
    let text = std::fs::read_to_string(e.path("state/snapshots/spawnsnap.imux")).unwrap_or_default();
    t.ok("the snapshot holds who made which, by position", text.contains("meta imux.spawn "));
    e.kill_server();

    t.section("after a restore the numbers are new and the makers are the same");
    let mut c = e.client(100, 30, &["new", "-s", "fresh"]);
    assert!(e.has_client(), "the client did not attach");
    assert!(c.wait_drawn(LONG), "the client drew nothing");
    let r = e.all(&["restore", "spawnsnap"]);
    t.matches("restore brings the session back", "restored 1 session", &r);
    let panes = e.out(&["list-panes", "-a", "-F", "#{session_name} #{window_index} #{pane_id}"]);
    let at = |w: &str| panes.lines().find_map(|l| l.strip_prefix(&format!("sp {w} "))).unwrap_or("").to_string();
    let (parent, child, grand, user) = (at("0"), at("1"), at("2"), at("3"));
    t.ok("the four panes are back", [&parent, &child, &grand, &user].iter().all(|id| id.starts_with('%')));
    t.ok("the records are back: the second was made by the first", e.var(&child, "#{pane_spawned_by}") == parent);
    t.ok("the third by the second", e.var(&grand, "#{pane_spawned_by}") == child);
    t.ok("the user's pane has no maker", e.var(&user, "#{pane_spawned_by}").is_empty());

    t.section("the program acts on its panes again, and on no others");
    let pn = parent.trim_start_matches('%').to_string();
    // the new numbers may be the old ones again: nothing of the first holder may be left
    for stale in [format!("fifo{pn}"), format!("ready{pn}")] {
        let _ = std::fs::remove_file(e.path(&stale));
    }
    hold(&e, &pn);
    let (rc, o) = in_pane(&e, &pn, &format!("{b} send-keys -l -t {grand} hi"));
    t.ok("it can act on a pane it spawned, through the one in between", rc == 0 && !o.contains("spawned"));
    let (rc, o) = in_pane(&e, &pn, &format!("{b} send-keys -l -t {user} hi"));
    t.ok("and not on a pane the user made", rc != 0 && o.contains("spawned"));
    let (rc, _) = in_pane(&e, &pn, &format!("{b} kill-pane -t {child}"));
    t.ok("it can close a pane it spawned", rc == 0);
    c.send(b"\x02d");
    c.wait_exit(LONG);
    e.kill_server();
    t.finish();
}
