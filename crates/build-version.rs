// SPDX-License-Identifier: MPL-2.0

// Included by the front ends' build scripts (mothrix-tui, mothrix-ffi):
// sets MOTHRIX_VERSION, the version the user is shown. A release built at
// its tag (`v0.2.0`, clean) is just "0.2.0"; any other build adds the
// commit it was made from, "0.2.0 (ce613f6)", and "-dirty" when files
// differ from it. Without git (a source archive) it is the bare version.

use std::{path::Path, process::Command};

fn emit_version() {
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets it");
    let text = match commit(&version) {
        Some(commit) => format!("{version} ({commit})"),
        None => version,
    };
    println!("cargo:rustc-env=MOTHRIX_VERSION={text}");

    // Again when the sources change (for "-dirty"), and when a commit,
    // a checkout or a tag moves what git calls HEAD.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    println!("cargo:rerun-if-changed={}", crates.display());
    println!("cargo:rerun-if-changed={}", crates.join("../Cargo.toml").display());
    if let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        for name in ["HEAD", "index", "refs", "packed-refs"] {
            println!("cargo:rerun-if-changed={git_dir}/{name}");
        }
    }
}

/// The commit to name, if this is not a clean build of the release's tag.
fn commit(version: &str) -> Option<String> {
    let hash = git(&["rev-parse", "--short", "HEAD"])?;
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|changes| !changes.is_empty());
    let tag = format!("v{version}");
    let tagged = git(&["tag", "--points-at", "HEAD"])
        .is_some_and(|tags| tags.lines().any(|line| line == tag));
    match (tagged, dirty) {
        (true, false) => None,
        (_, true) => Some(format!("{hash}-dirty")),
        (false, false) => Some(hash),
    }
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
