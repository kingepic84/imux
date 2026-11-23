//! imux spells color, center and their relatives the American way; the spellings tmux uses
//! (clock-mode-colour, status-justify centre, #[align=centre], popup_centre_x, colourN) keep working,
//! so an existing tmux configuration loads the same. (The Rust version of tests/q_american.sh.)
use crate::support::*;

#[test]
fn q_american() {
    let mut e = Env::new("qam");
    let conf = e.write("imux.conf", "set -g status off\n");
    e.set("IMUX_CONFIG", &p(&conf));
    let mut t = Check::new("q_american");
    e.session("am", "w");

    t.section("option names");
    t.ok("the American option name is accepted", e.run(&["set", "-g", "clock-mode-color", "red"]).success());
    t.ok("and shows its value", e.out(&["show-options", "-gv", "clock-mode-color"]) == "red");
    t.ok("the old name is accepted", e.run(&["set", "-g", "clock-mode-colour", "green"]).success());
    t.ok("and is the same option", e.out(&["show-options", "-gv", "clock-mode-color"]) == "green");
    t.ok("an ignored option answers to both spellings", e.run(&["set", "-g", "pane-colours", "1 2"]).success() && e.run(&["set", "-g", "pane-colors", "1 2"]).success());

    t.section("values");
    t.ok("status-justify center", e.run(&["set", "-g", "status-justify", "center"]).success());
    t.ok("status-justify centre", e.run(&["set", "-g", "status-justify", "centre"]).success());
    t.ok("status-justify absolute-center", e.run(&["set", "-g", "status-justify", "absolute-center"]).success());
    t.ok("colorN and colourN are one color", e.run(&["set", "-g", "status-style", "fg=color33"]).success() && e.run(&["set", "-g", "status-style", "fg=colour33"]).success());

    t.section("formats (the popup_center_x variables are covered by a unit test in server/position.rs)");
    t.ok("a status row accepts align=center and align=centre", e.run(&["set", "-g", "status-format[0]", "#[align=left]L#[align=center]M#[align=centre]N#[align=right]R"]).success());
    e.kill_server();
    t.finish();
}
