//! The core's version names its code: the package version and a fingerprint of every source file of the core, so
//! a change to the rules is a change of engine, and a run stays on the rules it started with.

use std::path::Path;

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn main() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    println!("cargo:rerun-if-changed={}", src.display());
    let mut all = Vec::new();
    files(&src, &mut all);
    all.sort();
    // FNV-1a over each file's path inside src and its bytes
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in &all {
        let name = f.strip_prefix(&src).map(|p| p.display().to_string()).unwrap_or_default();
        for b in name.bytes().chain([0u8]).chain(std::fs::read(f).unwrap_or_default()).chain([0u8]) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    println!("cargo:rustc-env=JAYNES_CORE_FINGERPRINT={h:016x}");
}
