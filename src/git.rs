//! Code storage is git: one repository per site, shared by every kernel
//! there. Each process is a branch, `name@kernel`, checked out in a working
//! directory of its own (a git worktree) kept by its kernel. Experts are
//! branches with plain names.
//!
//! Many kernels use the repository at once. That is safe because each writes
//! only its own branches, and because git's housekeeping, which is the one
//! thing that could collide, is never started from here.

use crate::index::Res;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn run(mut command: Command, args: &[&str], input: Option<&str>) -> Res<String> {
    command
        .args(["-c", "gc.auto=0"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not run git: {e}"))?;
    if let Some(input) = input {
        let _ = child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(input.as_bytes());
    }
    drop(child.stdin.take());
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("git {} failed: {}", args.join(" "), err.trim()).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Run git in a working directory and return what it printed.
pub fn git(dir: &Path, args: &[&str]) -> Res<String> {
    let mut command = Command::new("git");
    command.current_dir(dir);
    run(command, args, None)
}

/// The repository itself, wherever its working directories are.
#[derive(Clone)]
pub struct Repo {
    /// Where git keeps the repository (`.git`, or a bare repository).
    pub git_dir: PathBuf,
    /// The directory people think of as the repository: the one holding
    /// `.git`, or the bare repository itself.
    pub root: PathBuf,
}

impl Repo {
    /// The repository `dir` is in, also when `dir` is one of its working directories.
    pub fn find(dir: &Path) -> Res<Repo> {
        let common = git(dir, &["rev-parse", "--git-common-dir"]).map_err(|_| {
            format!(
                "{} is not in a git repository (run `git init` there first)",
                dir.display()
            )
        })?;
        let git_dir = fs_absolute(&dir.join(common));
        let root = match git_dir.file_name() {
            Some(name) if name == ".git" => git_dir
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or(git_dir.clone()),
            _ => git_dir.clone(),
        };
        Ok(Repo { git_dir, root })
    }

    pub fn git(&self, args: &[&str]) -> Res<String> {
        self.git_in(args, None)
    }

    fn git_in(&self, args: &[&str], input: Option<&str>) -> Res<String> {
        let mut command = Command::new("git");
        command
            .arg("--git-dir")
            .arg(&self.git_dir)
            .current_dir(&self.root);
        run(command, args, input)
    }

    /// The commit a name stands for, if git knows it.
    pub fn commit_of(&self, what: &str) -> Option<String> {
        self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{what}^{{commit}}"),
        ])
        .ok()
    }

    /// Get one branch from the remote `origin`, as `origin/BRANCH`. Never asks
    /// for a password and does not wait long for a remote that is not there.
    pub fn fetch(&self, branch: &str) -> Res<()> {
        let mut command = Command::new("git");
        command
            .arg("--git-dir")
            .arg(&self.git_dir)
            .current_dir(&self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env(
                "GIT_SSH_COMMAND",
                "ssh -o BatchMode=yes -o ConnectTimeout=10",
            );
        run(
            command,
            &[
                "fetch",
                "--quiet",
                "origin",
                &format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"),
            ],
            None,
        )
        .map(|_| ())
    }

    pub fn has_branch(&self, branch: &str) -> bool {
        self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .is_ok()
    }

    /// Take a kernel's name in this repository, for good. Nothing else may be
    /// a kernel of that name here afterwards; the same kernel may claim again.
    pub fn claim(&self, name: &str, identity: &str) -> Res<()> {
        let claim = format!("refs/jarvis/kernels/{name}");
        let blob = self.git_in(&["hash-object", "-w", "--stdin"], Some(identity))?;
        // The empty old value means: only if nobody has it. Git does this in one step.
        if self.git(&["update-ref", &claim, &blob, ""]).is_ok() {
            return Ok(());
        }
        match self.git(&["cat-file", "-p", &claim]) {
            Ok(holder) if holder.trim() == identity => Ok(()),
            Ok(_) => Err(format!(
                "there has already been a kernel called `{name}` in this repository, and this is not it. Choose a name that was never used \
                 (what that kernel's processes had is on the branches `...@{name}`)."
            )
            .into()),
            Err(e) => Err(format!("could not claim the name `{name}` in the repository: {e}").into()),
        }
    }
}

fn fs_absolute(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
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
        Ok(current) => Some(format!(
            "the directory is on branch `{current}`, not on `{branch}`"
        )),
        Err(_) => Some(format!(
            "the directory is on no branch (detached), not on `{branch}`"
        )),
    }
}

/// Commit everything in a working directory, with `author` as the name on
/// the commit. Returns the new commit, or `None` when nothing had changed.
pub fn commit_all(dir: &Path, author: &str, message: &str) -> Res<Option<String>> {
    git(dir, &["add", "-A"])?;
    if git(dir, &["status", "--porcelain"])?.is_empty() {
        return Ok(None);
    }
    git(
        dir,
        &[
            "-c",
            &format!("user.name={author}"),
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    )?;
    Ok(head(dir))
}

/// Commit even though nothing changed. A new process always gets a first
/// commit, so that its branch exists from the moment it does.
pub fn commit_empty(dir: &Path, author: &str, message: &str) -> Res<()> {
    git(
        dir,
        &[
            "-c",
            &format!("user.name={author}"),
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            message,
        ],
    )?;
    Ok(())
}

/// How a process's working directory comes to be.
pub enum Start<'a> {
    /// A new branch with no history.
    Nothing,
    /// A new branch starting at a commit. It shares that history: a fork.
    From(&'a str),
    /// The branch exists already; carry on from it.
    Existing,
}

/// Check out `branch` at `path`. The working directory is marked as locked:
/// it may be on a disk other machines cannot see, and git would otherwise
/// take it for deleted and forget it.
pub fn worktree_add(repo: &Repo, path: &Path, branch: &str, start: Start) -> Res<()> {
    let path = path.to_string_lossy();
    match start {
        Start::Nothing => repo.git(&[
            "worktree", "add", "--quiet", "--lock", "--orphan", "-b", branch, &path,
        ])?,
        Start::From(commit) => repo.git(&[
            "worktree", "add", "--quiet", "--lock", "-b", branch, &path, commit,
        ])?,
        Start::Existing => repo.git(&["worktree", "add", "--quiet", "--lock", &path, branch])?,
    };
    Ok(())
}

pub fn worktree_remove(repo: &Repo, path: &Path) -> Res<()> {
    let path = path.to_string_lossy();
    let _ = repo.git(&["worktree", "unlock", &path]);
    repo.git(&["worktree", "remove", "--force", &path])?;
    Ok(())
}
