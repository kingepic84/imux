//! `scripts/check-surface.py`: the version must have been raised as far as what changed in the
//! user-facing surface (keys, options and their defaults, commands, flags, API and snapshot
//! versions) needs, compared with the surface of the last release in `surface/`.
use crate::support::*;
use std::path::Path;
use std::process::Command;

const BASE: &str = "[keys]\nprefix c\tnew-window\nprefix x\tkill-pane\n[options]\nautosave-interval\t300\nstatus\ton\n[commands]\nkill-server\nnew-session\n[usage]\n-s\nname\n[versions]\napi\tapi=1\nsnapshot\timux-snapshot 1\n";

fn script() -> String {
    format!("{}/scripts/check-surface.py", env!("CARGO_MANIFEST_DIR"))
}

/// The verdict for two surfaces and the versions they belong to.
fn compare(dir: &Path, base: &str, new: &str, base_v: &str, new_v: &str) -> (bool, String) {
    let (b, n) = (dir.join("base.txt"), dir.join("new.txt"));
    std::fs::write(&b, base).unwrap();
    std::fs::write(&n, new).unwrap();
    let out = Command::new("python3").arg(script()).args(["--compare", &p(&b), &p(&n), base_v, new_v]).output().expect("python3 runs");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn q_surface() {
    if which("python3").is_none() {
        println!("skip: python3 is not installed");
        return;
    }
    let e = Env::new("qsf");
    let mut t = Check::new("q_surface");
    let d = e.dir.clone();

    t.section("nothing changed");
    t.ok("the same surface passes at the same version", compare(&d, BASE, BASE, "0.1.0", "0.1.0").0);
    t.ok("and at a patch version", compare(&d, BASE, BASE, "0.1.0", "0.1.1").0);
    t.ok("and at a higher major", compare(&d, BASE, BASE, "0.1.0", "1.0.0").0);

    t.section("something added");
    let added = BASE.replace("[commands]\n", "[commands]\nrename-pane\n");
    let (ok, out) = compare(&d, BASE, &added, "0.1.0", "0.1.1");
    t.ok("a new command needs more than a patch", !ok);
    t.matches("and the new word is named", "commands: rename-pane is new", &out);
    t.ok("a minor version is enough", compare(&d, BASE, &added, "0.1.0", "0.2.0").0);
    t.ok("also from 1.0", compare(&d, BASE, &added, "1.0.0", "1.1.0").0);
    t.ok("a new option is an addition too", !compare(&d, BASE, &BASE.replace("status\ton\n", "status\ton\nstatus-new\toff\n"), "0.1.0", "0.1.1").0);
    t.ok("a new key is one", !compare(&d, BASE, &BASE.replace("prefix x\tkill-pane\n", "prefix x\tkill-pane\nprefix y\tzoom\n"), "1.2.0", "1.2.1").0);
    t.ok("a new flag word is one", !compare(&d, BASE, &BASE.replace("[usage]\n", "[usage]\n--history\n"), "1.2.0", "1.2.1").0);

    t.section("something taken away or changed");
    let no_key = BASE.replace("prefix x\tkill-pane\n", "");
    let (ok, out) = compare(&d, BASE, &no_key, "0.1.0", "0.1.1");
    t.ok("a key that is gone is a break, and a patch is not enough", !ok);
    t.matches("and the key is named", "keys: prefix x is gone", &out);
    t.ok("while the major version is 0 a minor one is enough", compare(&d, BASE, &no_key, "0.1.0", "0.2.0").0);
    t.ok("from 1.0 a minor version is not enough", !compare(&d, BASE, &no_key, "1.0.0", "1.1.0").0);
    t.ok("a major one is", compare(&d, BASE, &no_key, "1.0.0", "2.0.0").0);
    let (ok, out) = compare(&d, BASE, &BASE.replace("autosave-interval\t300", "autosave-interval\t60"), "1.0.0", "1.0.1");
    t.ok("a changed default is a break", !ok);
    t.matches("and says what it was and is", "was '300' and is now '60'", &out);
    t.ok("a key bound to another command is a break", !compare(&d, BASE, &BASE.replace("kill-pane", "kill-window"), "1.0.0", "1.1.0").0);
    t.ok("an option that is gone is a break", !compare(&d, BASE, &BASE.replace("status\ton\n", ""), "1.0.0", "1.1.0").0);
    t.ok("a command that is gone is a break", !compare(&d, BASE, &BASE.replace("kill-server\n", ""), "1.0.0", "1.9.9").0);
    t.ok("a flag that is gone is a break", !compare(&d, BASE, &BASE.replace("-s\n", ""), "1.0.0", "1.1.0").0);
    t.ok("a new API version is a break", !compare(&d, BASE, &BASE.replace("api=1", "api=2"), "1.0.0", "1.1.0").0);
    t.ok("a new snapshot version is a break", !compare(&d, BASE, &BASE.replace("imux-snapshot 1", "imux-snapshot 2"), "1.0.0", "1.1.0").0);
    t.ok("an addition and a break together need what the break needs", !compare(&d, BASE, &no_key.replace("[commands]\n", "[commands]\nrename-pane\n"), "1.0.0", "1.1.0").0);

    t.section("the version below the release");
    let (ok, out) = compare(&d, BASE, BASE, "0.2.0", "0.1.9");
    t.ok("is refused", !ok);
    t.matches("and says so", "below the release", &out);

    t.section("this repository, with the imux that was built");
    if cfg!(target_os = "linux") {
        // a few defaults differ per system, and the baselines are made on Linux
        let out = Command::new("python3").arg(script()).args(["--binary", env!("CARGO_BIN_EXE_imux")]).current_dir(env!("CARGO_MANIFEST_DIR")).output().expect("python3 runs");
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        t.ok("the version fits the surface against the last release", out.status.success());
        if !out.status.success() {
            println!("{text}");
        }
        t.matches("and says which release it was compared with", "fits the surface against v[0-9]", &text);
    } else {
        println!("skip: the baselines are made on Linux");
    }
    t.finish();
}
