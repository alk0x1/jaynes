//! The commit the program is built from, so `jaynes --version`, the page and `jaynes update` say what runs: the
//! release names it in JAYNES_BUILD_COMMIT, else git is asked where the source is, and a build with changes not yet
//! committed says so. A source with no git, as a crate, says "unknown".

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(env!("CARGO_MANIFEST_DIR")).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=JAYNES_BUILD_COMMIT");
    // a new commit, a checkout or a change staged builds again; a change to the source does anyway
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        for f in ["HEAD", "logs/HEAD", "index"] {
            println!("cargo:rerun-if-changed={dir}/{f}");
        }
    }
    let engine = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    for d in ["crates/core/src", "crates/shell/src", "python"] {
        println!("cargo:rerun-if-changed={engine}/{d}");
    }
    let commit = match std::env::var("JAYNES_BUILD_COMMIT") {
        Ok(c) if !c.trim().is_empty() => c.trim().to_string(),
        _ => match git(&["rev-parse", "HEAD"]) {
            Some(head) => {
                let changed = git(&["status", "--porcelain", "--untracked-files=no", "--", engine]).is_some_and(|s| !s.is_empty());
                if changed { format!("{head}+changes") } else { head }
            }
            None => "unknown".to_string(),
        },
    };
    println!("cargo:rustc-env=JAYNES_BUILD_COMMIT={commit}");
}
