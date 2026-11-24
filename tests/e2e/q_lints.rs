//! The clippy lints that a newer clippy no longer turns on by itself stay on: `overly_complex_bool_expr`
//! (a correctness lint until clippy 1.97 made it pedantic) is denied in `Cargo.toml`, so a condition
//! such as `x || true` fails the lint check whichever clippy runs, and `nonminimal_bool` (moved with it).
use crate::support::*;

#[test]
fn q_lints() {
    let mut t = Check::new("q_lints");
    let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap_or_default();
    let section = toml.split("[lints.clippy]").nth(1).and_then(|rest| rest.split("\n[").next()).unwrap_or("");
    t.ok("Cargo.toml has a [lints.clippy] table", toml.contains("[lints.clippy]"));
    t.matches("with overly_complex_bool_expr denied", "(?m)^overly_complex_bool_expr = \"deny\"$", section);
    t.matches("and nonminimal_bool on (also moved to pedantic in clippy 1.97)", "(?m)^nonminimal_bool = \"warn\"$", section);
    t.finish();
}
