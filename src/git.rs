//! Code storage is git. Each process is a branch, checked out in its own
//! working directory (a git worktree) while the process exists in this home.

use crate::db::Res;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run git in `dir` and return what it printed.
pub fn git(dir: &Path, args: &[&str]) -> Res<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), err.trim()).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The commit a working directory is at, if it has one.
pub fn head(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "HEAD"]).ok()
}

/// Why the kernel should leave this working directory alone for now, if it
/// should: an operation such as a merge is half done, or the directory is not
/// on the process's own branch. Committing in either state would record
/// something nobody meant.
pub fn not_committable(dir: &Path, branch: &str) -> Option<String> {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]).ok()?);
    let unfinished = [
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
    ];
    for (file, what) in unfinished {
        if git_dir.join(file).exists() {
            return Some(format!("a {what} is unfinished"));
        }
    }
    match git(dir, &["symbolic-ref", "--short", "-q", "HEAD"]) {
        Ok(current) if current == branch => None,
        Ok(current) => Some(format!("the directory is on branch `{current}`, not on `{branch}`")),
        Err(_) => Some(format!("the directory is on no branch (detached), not on `{branch}`")),
    }
}

/// Commit everything in a working directory, with `author` as the name on
/// the commit. Returns the new commit, or `None` when nothing had changed.
pub fn commit_all(dir: &Path, author: &str, message: &str) -> Res<Option<String>> {
    git(dir, &["add", "-A"])?;
    if git(dir, &["status", "--porcelain"])?.is_empty() {
        return Ok(None);
    }
    git(dir, &["-c", &format!("user.name={author}"), "commit", "--quiet", "-m", message])?;
    Ok(head(dir))
}

/// Commit even though nothing changed. A new process always gets a first
/// commit, so that its branch exists from the moment it does.
pub fn commit_empty(dir: &Path, author: &str, message: &str) -> Res<()> {
    git(dir, &["-c", &format!("user.name={author}"), "commit", "--quiet", "--allow-empty", "-m", message])?;
    Ok(())
}

/// A new branch with no history, checked out at `path`.
pub fn worktree_new(root: &Path, path: &Path, branch: &str) -> Res<()> {
    git(root, &["worktree", "add", "--quiet", "--orphan", "-b", branch, &path.to_string_lossy()])?;
    Ok(())
}

/// A new branch starting from `start` (another process's branch), checked
/// out at `path`. The new branch shares the old one's history: this is a fork.
pub fn worktree_fork(root: &Path, path: &Path, branch: &str, start: &str) -> Res<()> {
    git(root, &["worktree", "add", "--quiet", "-b", branch, &path.to_string_lossy(), start])?;
    Ok(())
}
