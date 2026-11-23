//! Format conditionals and comparisons (M34): #{?cond,a,b}, ==, !=, <, >, &&, ||, !, escapes,
//! through display-message -p, list-windows -F and the status bar of a config.
//! (The Rust version of tests/f_format.sh.)
use crate::support::*;
use std::time::Duration;

#[test]
fn f_format() {
    let mut e = Env::new("ffmt");
    let mut t = Check::new("f_format");
    e.session("f", "alpha");
    e.run(&["new-window", "-t", "f", "-n", "beta"]);
    let is = |t: &mut Check, label: &str, format: &str, want: &str| {
        let got = e.var("f:", format);
        t.eq(label, got, want.to_string());
    };
    t.section("conditionals");
    is(&mut t, "true condition picks then", "#{?window_active,yes,no}", "yes");
    is(&mut t, "a word that is no variable counts as text, so true", "#{?pane_title_nothing_here,yes,no}", "yes");
    is(&mut t, "a zero is false", "#{?#{==:1,2},yes,no}", "no");
    is(&mut t, "a variable that is 0 is false", "#{?session_attached,attached,detached}", "detached");
    is(&mut t, "else may be empty", "#{?#{==:1,2},yes,}", "");
    is(&mut t, "no else gives nothing", "#{?#{==:1,2},yes}", "");
    is(&mut t, "pairs chain", "#{?#{==:1,2},a,#{==:1,1},b,c}", "b");
    is(&mut t, "branches expand", "#{?window_active,[#{window_name}],-}", "[beta]");
    t.section("comparisons and logic");
    is(&mut t, "== equal", "#{==:#{window_name},beta}", "1");
    is(&mut t, "== not equal", "#{==:#{window_name},alpha}", "0");
    is(&mut t, "!= ", "#{!=:#{window_name},alpha}", "1");
    is(&mut t, "text order <", "#{<:a,b}", "1");
    is(&mut t, "text order >", "#{>:a,b}", "0");
    is(&mut t, "&& with three operands", "#{&&:1,1,1}", "1");
    is(&mut t, "&& with a zero", "#{&&:1,0,1}", "0");
    is(&mut t, "|| ", "#{||:0,,1}", "1");
    is(&mut t, "! ", "#{!:0}", "1");
    is(&mut t, "nested", "#{?#{&&:#{==:#{window_name},beta},#{!:0}},both,not}", "both");
    t.section("escapes and malformed input");
    is(&mut t, "an escaped comma stays in the operand", "#{?1,a#,b,c}", "a,b");
    is(&mut t, "an escaped brace", "#{?1,{#},c}", "{}");
    is(&mut t, "a hash stays a hash", "#{?1,##,c}", "#");
    is(&mut t, "malformed ? is left as written", "#{?1}", "#{?1}");
    is(&mut t, "malformed == is left as written", "#{==:a}", "#{==:a}");
    t.section("list-windows -F");
    let lw = e.all(&["list-windows", "-t", "f", "-F", "#{?window_active,*,-}#{window_name}"]);
    t.ok("each window is formatted by its own state", lw.lines().any(|l| l == "-alpha") && lw.lines().any(|l| l == "*beta"));
    t.section("the status bar");
    e.kill_server();
    let sc = e.write(
        "status.conf",
        "set -g status-right \"#{?#{==:#{session_name},f},IN-F,OUT}\"\nset -g window-status-format \"#{?window_active,[#W],#W}\"\nset -g window-status-current-format \"#{?window_active,[#W],#W}\"\n",
    );
    e.set("IMUX_CONFIG", &p(&sc));
    let mut c = e.client(80, 24, &["new", "-s", "f"]);
    c.wait_seen("IN-F", Duration::from_secs(10));
    e.barrier(&c, "f");
    let text = c.text();
    t.ok("status-right takes a comparison", text.contains("IN-F"));
    t.ok("the current window shows its brackets", text.contains('['));
    c.kill();
    e.kill_server();
    t.finish();
}
