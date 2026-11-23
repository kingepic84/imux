//! monitor-silence and silence-action: a window nobody views that printed nothing for the given
//! seconds gets the ~ flag (and #{window_silence_flag}); printing again takes it off; 0 turns the
//! monitor off. (The Rust version of tests/q_silence.sh.)
use crate::support::*;
use regex_lite::Regex;
use std::time::Duration;

#[test]
fn q_silence() {
    let e = Env::new("qsil");
    let mut t = Check::new("q_silence");
    let flags = || e.all(&["list-windows", "-t", "q", "-F", "#{window_name}=#{window_flags}"]);
    let until_flags = |pattern: &str| {
        let re = Regex::new(&format!("(?m){pattern}")).unwrap();
        wait_until(Duration::from_secs(10), || re.is_match(&flags()))
    };
    e.session("q", "cur");
    e.run(&["new-window", "-d", "-t", "q", "-n", "quiet"]);
    e.ready("q:quiet", Duration::from_millis(400));
    t.ok("off by default: no flag", !flags().contains('~'));
    let evs = p(&e.path("silence.log"));
    e.run(&["set-hook", "-g", "alert-silence", &format!("run-shell \"echo silence:#{{q:hook_window_name}} >> {evs}\"")]);
    e.run(&["set", "-g", "monitor-silence", "2"]);
    t.ok("monitor-silence is a real option", e.out(&["show-options", "-gv", "monitor-silence"]) == "2");
    t.ok("a quiet window gets the flag", until_flags("quiet=.*~"));
    t.ok("the variable says so", e.var("q:quiet", "#{window_silence_flag}") == "1");
    t.ok("the alert-silence hook ran for that window", wait_until(Duration::from_secs(5), || e.read("silence.log").lines().any(|l| l == "silence:quiet")));
    t.ok("the current window is left alone with the default silence-action", !Regex::new("(?m)cur=.*~").unwrap().is_match(&flags()));
    e.run(&["send-keys", "-t", "q:quiet", "echo noise", "Enter"]);
    t.ok("printing takes the flag off", until_flags("quiet=[^~]*$"));
    t.ok("and it comes back after the quiet time", until_flags("quiet=.*~"));
    e.run(&["set", "-g", "silence-action", "none"]);
    e.run(&["send-keys", "-t", "q:quiet", "echo again", "Enter"]);
    // monitor-silence is 2 s: a flag, were one to be set, would come two seconds after the output, so
    // this watches for a second and a half longer than that (nothing shows that it is not coming)
    std::thread::sleep(Duration::from_millis(3500));
    t.ok("silence-action none sets no flag", !Regex::new("(?m)quiet=.*~").unwrap().is_match(&flags()));
    e.run(&["set", "-g", "silence-action", "other"]);
    e.run(&["set", "-g", "monitor-silence", "0"]);
    t.ok("monitor-silence 0 takes the flag away", wait_until(Duration::from_secs(10), || !flags().contains('~')));
    e.kill_server();
    t.finish();
}
