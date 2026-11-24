//! The reminder that `imux setup` has not been run: a box over the panes when a client attaches, with
//! the command to run. It closes with Escape (or `q`), and a version of imux shows it once, so it
//! never turns into a nag. What decides it is `setup::unfinished`: the shell's startup file has no
//! `imux init` line, `imux setup` has never run, and the person has not turned the reminder off.

use super::Server;
use crate::input::bindings::{MenuItemSpec, MenuSpec};
use crate::{log, persist, setup};

impl Server {
    /// Open the reminder for a client that just attached, if setup is unfinished and this version
    /// has not shown it yet. A box that does not fit, or no terminal size yet, shows nothing.
    pub(super) fn setup_reminder(&mut self, id: usize) {
        let env = |k: &str| std::env::var(k).ok();
        let state = persist::state_dir(|k| std::env::var_os(k)).ok();
        let Some(rc) = setup::unfinished(&env, state.as_deref()) else { return };
        if state.as_deref().is_some_and(setup::seen_by_this_version) {
            return;
        }
        let shown = setup::shown_path(&rc, env("HOME").as_deref());
        let text = |line: &str| MenuItemSpec { name: format!("-{line}"), key: String::new(), command: String::new() };
        let rule = || MenuItemSpec { name: String::new(), key: String::new(), command: String::new() };
        let spec = MenuSpec {
            title: " Finish setting up imux ".to_string(),
            x: Some("C".to_string()),
            y: Some("C".to_string()),
            items: vec![
                text("Run this once, in a shell:"),
                text(" "),
                text("    imux setup"),
                text(" "),
                text(&format!("It adds one line to {shown}, and installs")),
                text("the manual, the agent skill, and the Claude Code hook."),
                text("Without it programs in your panes may not find imux's"),
                text("link, and agents do not learn how to use it."),
                rule(),
                MenuItemSpec { name: "Close (Esc)".to_string(), key: "q".to_string(), command: String::new() },
            ],
            ..MenuSpec::default()
        };
        self.open_menu(id, &spec);
        if let Some(dir) = state {
            setup::mark_seen(&dir);
        }
        log::info(&format!("client {id}: the setup reminder was shown ({shown} has no imux init line)"));
    }
}
