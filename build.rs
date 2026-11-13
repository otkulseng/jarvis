//! Tells the program which commit it was built from, for `jarvis --version`
//! and for what a kernel says of itself.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let commit = git(&["rev-parse", "--short", "HEAD"])
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".into());
    // Built from files that differ from that commit.
    let changed =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    println!(
        "cargo:rustc-env=JARVIS_BUILD={commit}{}",
        if changed { "+changes" } else { "" }
    );
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
