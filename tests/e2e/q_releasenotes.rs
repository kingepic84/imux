//! A release has notes: `scripts/check-release-notes.py` refuses a tag with no file in
//! `release-notes/`, an empty or too short one, one with a placeholder, and one that is not text;
//! the release workflow runs it before anything is built and publishes that file as the text of the
//! release, and the CI runs it on a tag; the releases so far have their notes.
use crate::support::*;
use std::path::Path;
use std::process::Command;

fn script() -> String {
    format!("{}/scripts/check-release-notes.py", env!("CARGO_MANIFEST_DIR"))
}

fn check(dir: &Path, tag: &str) -> (bool, String) {
    let out = Command::new("python3").arg(script()).args(["--dir", &p(dir), tag]).output().expect("python3 runs");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn q_releasenotes() {
    if which("python3").is_none() {
        println!("skip: python3 is not installed");
        return;
    }
    let e = Env::new("qrn");
    let mut t = Check::new("q_releasenotes");
    let d = e.path("notes");
    std::fs::create_dir_all(&d).unwrap();
    let good = format!("A bug-fix release.\n\n## Fixed\n\n- {}\n\n## Install or upgrade\n\n`brew upgrade kingepic84/tap/imux`, then run `imux setup`.\n", "Something that was wrong now works. ".repeat(4));

    t.section("what is refused");
    let (ok, out) = check(&d, "v1.2.3");
    t.ok("no file", !ok);
    t.matches("and it says which file to write", "write .*v1\\.2\\.3\\.md before tagging v1\\.2\\.3", &out);
    std::fs::write(d.join("v1.2.3.md"), "").unwrap();
    t.ok("an empty file", !check(&d, "v1.2.3").0);
    std::fs::write(d.join("v1.2.3.md"), "Fixed a bug.\n").unwrap();
    let (ok, out) = check(&d, "v1.2.3");
    t.ok("a file that says too little", !ok);
    t.matches("and says how little", "says too little", &out);
    std::fs::write(d.join("v1.2.3.md"), format!("{good}\nTODO: the upgrade steps\n")).unwrap();
    let (ok, out) = check(&d, "v1.2.3");
    t.ok("a placeholder", !ok);
    t.matches("and names it", "still says TODO", &out);
    std::fs::write(d.join("v1.2.3.md"), b"\xff\xfe not text".repeat(40)).unwrap();
    t.ok("a file that is not UTF-8 text", !check(&d, "v1.2.3").0);
    t.ok("a name that is not a version tag", !check(&d, "1.2.3").0 && !check(&d, "v1.2").0 && !check(&d, "../etc").0);

    t.section("what is accepted");
    std::fs::write(d.join("v1.2.3.md"), &good).unwrap();
    let (ok, out) = check(&d, "v1.2.3");
    t.ok("notes that say what changed", ok);
    t.matches("and it says so", "v1\\.2\\.3 has release notes", &out);
    std::fs::write(d.join("v1.2.4-rc.1.md"), &good).unwrap();
    t.ok("a pre-release tag", check(&d, "v1.2.4-rc.1").0);

    t.section("this repository");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for tag in ["v0.1.0", "v0.1.1", "v0.2.0"] {
        let (ok, _) = check(&root.join("release-notes"), tag);
        t.ok(&format!("{tag} has its notes"), ok);
    }
    let release = std::fs::read_to_string(root.join(".forgejo/workflows/release.yml")).unwrap_or_default();
    t.ok("the release workflow checks the notes before it builds", release.find("check-release-notes.py").is_some_and(|c| c < release.find("scripts/release.sh").unwrap_or(0)));
    t.ok("publishes the file as the text of the release, and refuses without it", release.contains("release-notes/$tag.md") && release.contains("not publishing"));
    let ci = std::fs::read_to_string(root.join(".forgejo/workflows/ci.yml")).unwrap_or_default();
    t.ok("the CI checks the notes on a tag", ci.contains("check-release-notes.py"));
    t.finish();
}
