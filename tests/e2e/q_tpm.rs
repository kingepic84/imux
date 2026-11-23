//! `run` and the `tmux` shim: a plugin script started from the configuration (as tpm starts its
//! plugins) drives imux through `tmux set`, `bind-key`, `set-environment` and reads back through
//! `display-message` and `show-environment`. (The Rust version of tests/q_tpm.sh.)
use crate::support::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

#[test]
fn q_tpm() {
    let mut e = Env::new("qtpm");
    let mut t = Check::new("q_tpm");
    let d = p(&e.dir);
    e.set("PLUG_OUT", &d);
    let plugin = e.path("plugin.tmux");
    std::fs::copy(format!("{}/tests/fixtures/plugin.tmux", env!("CARGO_MANIFEST_DIR")), &plugin).unwrap();
    std::fs::set_permissions(&plugin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let conf = e.write("imux.conf", &format!("set -g @plugin 'someone/plugin'\nrun '{d}/plugin.tmux'\n"));
    e.set("IMUX_CONFIG", &p(&conf));
    e.session("tp", "start");
    let w = |f: &dyn Fn() -> bool| wait_until(Duration::from_secs(10), f);
    t.ok("the plugin script ran and set an option through the shim", w(&|| e.out(&["show-options", "-gv", "@plug-ran"]) == "yes"));
    t.ok("tmux -V inside the script is the shim's answer", w(&|| e.out(&["show-options", "-gv", "@plug-version"]).contains("tmux 3")));
    t.ok("a binding made by the script exists", w(&|| regex_lite::Regex::new("rename-window .plugged").unwrap().is_match(&e.out(&["list-keys"]))));
    t.ok("a root binding with a command and flags exists", w(&|| e.out(&["list-keys"]).contains("new-window -n from-plugin")));
    t.ok("the script read back its own option", w(&|| e.read("echo.out").trim() == "yes"));
    t.ok("set-environment and show-environment round trip", w(&|| e.read("env.out").contains("PLUG_ENV=one")));
    t.ok("the script has TMUX set", e.read("tmuxvar.out").trim() == "tmux-set");
    e.kill_server();
    t.finish();
}
