//! `scripts/check-version.py`, which CI runs on every push and every tag: the version must be a valid
//! semantic version that agrees with Cargo.lock and never goes below an earlier release, and a tag
//! must be exactly `v` plus the version and higher than every release before it.
use crate::support::*;
use std::path::Path;
use std::process::Command;

fn repo(dir: &Path, version: &str, locked: &str) {
    std::fs::create_dir_all(dir).unwrap();
    assert!(git(dir, &["init", "-q", "-b", "main"]));
    std::fs::write(dir.join("Cargo.toml"), format!("[package]\nname = \"imux\"\nversion = \"{version}\"\nedition = \"2024\"\n\n[dependencies]\nlibc = \"0.2\"\n")).unwrap();
    std::fs::write(dir.join("Cargo.lock"), format!("[[package]]\nname = \"libc\"\nversion = \"0.2.1\"\n\n[[package]]\nname = \"imux\"\nversion = \"{locked}\"\n")).unwrap();
    assert!(git(dir, &["add", "."]));
    assert!(git(dir, &["commit", "-q", "-m", "x"]));
}

/// The exit status and everything the check printed, run in `dir` with an optional tag to check.
fn check(dir: &Path, tag: Option<&str>) -> (bool, String) {
    let script = format!("{}/scripts/check-version.py", env!("CARGO_MANIFEST_DIR"));
    let mut c = Command::new("python3");
    c.arg(script).current_dir(dir);
    if let Some(t) = tag {
        c.arg(t);
    }
    let out = c.output().expect("python3 runs");
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

#[test]
fn q_semver() {
    if which("python3").is_none() {
        println!("skip: python3 is not installed");
        return;
    }
    let e = Env::new("qsv");
    let mut t = Check::new("q_semver");
    let n = std::cell::Cell::new(0);
    let fresh = |version: &str, locked: &str, tags: &[&str]| {
        n.set(n.get() + 1);
        let d = e.path(&format!("r{}", n.get()));
        repo(&d, version, locked);
        for tag in tags {
            assert!(git(&d, &["tag", tag]));
        }
        d
    };

    t.section("the version of the tree");
    let (ok, out) = check(&fresh("0.1.1", "0.1.1", &["v0.1.0"]), None);
    t.ok("a version above the last release passes", ok);
    t.matches("and says which one", "0.1.1 is a valid version", &out);
    t.ok("so does the same version as the last release (before the next bump)", check(&fresh("0.1.0", "0.1.0", &["v0.1.0"]), None).0);
    t.ok("so does a tree with no release yet", check(&fresh("0.1.0", "0.1.0", &[]), None).0);
    t.ok("so do pre-release and build versions", check(&fresh("1.0.0-rc.1+build.5", "1.0.0-rc.1+build.5", &["v0.9.0"]), None).0);
    let (ok, out) = check(&fresh("0.1", "0.1", &[]), None);
    t.ok("two numbers are not a semantic version", !ok);
    t.matches("and the reason is said", "not a semantic version", &out);
    t.ok("a leading zero is not one either", !check(&fresh("01.2.3", "01.2.3", &[]), None).0);
    t.ok("nor is a v in front", !check(&fresh("v1.2.3", "v1.2.3", &[]), None).0);
    let (ok, out) = check(&fresh("0.1.1", "0.1.0", &[]), None);
    t.ok("a lock file that disagrees fails", !ok);
    t.matches("and says to rebuild", "Cargo.lock has imux", &out);
    let (ok, out) = check(&fresh("0.1.0", "0.1.0", &["v0.2.0"]), None);
    t.ok("a version below a release fails", !ok);
    t.matches("and names the release", "lower than the release v0.2.0", &out);
    t.ok("a pre-release is below its release", !check(&fresh("1.0.0-rc.1", "1.0.0-rc.1", &["v1.0.0"]), None).0);
    t.ok("1.10.0 is above 1.9.0, as numbers", check(&fresh("1.10.0", "1.10.0", &["v1.9.0"]), None).0);
    t.ok("a release tag that is not a version fails", !check(&fresh("0.1.1", "0.1.1", &["v0.1.0", "v1.x"]), None).0);

    t.section("a tag");
    t.ok("v plus the version, higher than the one before, passes", check(&fresh("0.1.1", "0.1.1", &["v0.1.0", "v0.1.1"]), Some("v0.1.1")).0);
    t.ok("the first release passes", check(&fresh("0.1.0", "0.1.0", &["v0.1.0"]), Some("v0.1.0")).0);
    let (ok, out) = check(&fresh("0.1.1", "0.1.1", &["v0.1.0", "v0.1.2"]), Some("v0.1.2"));
    t.ok("a tag that is not the version of the tree fails", !ok);
    t.matches("and says what a release is", "a release is v plus the version", &out);
    let (ok, out) = check(&fresh("0.1.0", "0.1.0", &["v0.1.0"]), Some("v0.1.0"));
    t.ok("the first tag passes beside itself", ok);
    let _ = out;
    let (ok, out) = check(&fresh("0.1.0", "0.1.0", &["v0.1.0", "v0.1.0-again", "v0.2.0"]), Some("v0.1.0"));
    t.ok("a tag at or below another release fails", !ok);
    t.matches("and says to raise the version first", "not higher than the release", &out);
    t.finish();
}
