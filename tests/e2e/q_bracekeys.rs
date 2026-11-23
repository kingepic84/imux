//! A lone brace that is the key of `bind` or `unbind` (`unbind {`, `bind } swap-pane -D`) is a key and
//! not a block brace: it is read as a key, and the lines after it still count. A brace after the key
//! opens a block as before, and a stray one is still refused. (The Rust version of tests/q_bracekeys.sh.)
use crate::support::*;

#[test]
fn q_bracekeys() {
    let e = Env::new("qbk");
    let mut t = Check::new("q_bracekeys");
    let keys = e.write("keys.conf", "unbind }\nunbind {\nbind } swap-pane -D\nbind { swap-pane -U\nbind -T prefix x { new-window }\nset -g nosuchoption 1\n");
    let out = e.all(&["check-config", &p(&keys)]);
    t.ok("the braces are keys: nothing is refused", !out.contains("refused"));
    t.ok("the line after them is read, with its own number", out.contains("keys.conf:6: unknown-option: nosuchoption"));
    t.ok("all six lines are counted", out.lines().any(|l| l == "files: 1, lines: 6, problems: 1"));
    let stray = e.write("stray.conf", "bind x new-window }\nset -g nosuchoption 1\n");
    let out = e.all(&["check-config", &p(&stray)]);
    t.ok("a stray closing brace after the key is still refused", out.contains("stray.conf:1: refused: unexpected } with no { block open"));
    t.ok("and the line after it is still read", out.contains("stray.conf:2: unknown-option: nosuchoption"));
    let open = e.write("open.conf", "display {\nset -g nosuchoption 1\n");
    let out = e.all(&["check-config", &p(&open)]);
    t.ok("a brace after a command that takes no key still opens a block", out.contains("open.conf:1: refused: a { block has no matching }"));
    t.finish();
}
