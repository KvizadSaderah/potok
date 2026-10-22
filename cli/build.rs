//! Which commit the binary was built from. The version is in `Cargo.toml`, the
//! rest is known at build time only: HEAD's hash, whether the tree is dirty,
//! and where the source lies, so `potok update` finds what to rebuild.
//!
//! With no git (an archive, someone else's machine) there is simply no hash —
//! `potok --version` then prints a version alone rather than an invented hash.
use std::path::Path;
use std::process::Command;

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    // `cli/` → `potok/`: install.sh lies there, and `potok update` calls it.
    let src = Path::new(&manifest).parent().map(Path::to_path_buf).unwrap_or_else(|| manifest.clone().into());
    println!("cargo:rustc-env=POTOK_SRC={}", src.display());

    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&src)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    if let Some(hash) = git(&["rev-parse", "--short", "HEAD"]).filter(|h| !h.is_empty()) {
        // Edits in potok itself rather than elsewhere in the repository: a «+»
        // means this binary cannot be restored from the hash.
        let dirty = git(&["status", "--porcelain", "--", "."]).is_some_and(|out| !out.is_empty());
        println!("cargo:rustc-env=POTOK_COMMIT={hash}{}", if dirty { "+" } else { "" });
        // A new commit moves HEAD and the ref it looks at: rebuild by those, or
        // the hash in the binary lies after the next commit.
        if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
            let dir = Path::new(&dir);
            println!("cargo:rerun-if-changed={}", dir.join("HEAD").display());
            println!("cargo:rerun-if-changed={}", dir.join("packed-refs").display());
            if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
                println!("cargo:rerun-if-changed={}", dir.join(reference).display());
            }
        }
    }
}
