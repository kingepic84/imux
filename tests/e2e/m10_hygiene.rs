//! Source hygiene: no stray print macros in src/ (log through crate::log), a panic hook in main, and
//! the server's event loop guarding its work. (The Rust version of tests/m10_hygiene.sh.)
use crate::support::*;
use regex_lite::Regex;
use std::path::Path;

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn m10_hygiene() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut t = Check::new("m10_hygiene");
    let print = Regex::new(r"\b(println|eprintln|print|eprint|dbg)!").unwrap();
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let mut found = Vec::new();
    for f in &files {
        for (n, line) in std::fs::read_to_string(f).unwrap_or_default().lines().enumerate() {
            if print.is_match(line) {
                found.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
            }
        }
    }
    for line in &found {
        println!("{line}");
    }
    t.ok("no print macros in src", found.is_empty());
    let main = std::fs::read_to_string(root.join("src/main.rs")).unwrap_or_default();
    t.ok("panic hook installed in main", main.contains("set_hook") || main.contains("install_panic_hook"));
    let server = std::fs::read_to_string(root.join("src/server/mod.rs")).unwrap_or_default();
    t.ok("server event loop guards its work", server.matches("guarded(").count() > 0);
    t.finish();
}
