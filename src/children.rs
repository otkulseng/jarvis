//! The programs the kernel starts: each process's `.jarvis/run`, its
//! `.jarvis/handle`, and its agent's harness.
//!
//! Two rules keep them under control:
//!
//! - Each is started as the leader of its own process group, so it and
//!   everything it starts in turn can be ended together.
//! - Each is written to the `children` table while it runs, with its start
//!   time. A kernel that starts after another one died finds there what was
//!   left behind, and ends it before doing anything else.
//!
//! Signals are sent with the `kill` command and start times read with `ps`,
//! so the kernel needs nothing beyond the standard library for this.

use rusqlite::{Connection, params};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long a program gets to end by itself after being asked to.
const GRACE: Duration = Duration::from_secs(3);

/// When the program with this pid started, if there is one. A pid together
/// with its start time names one program for good; a pid alone may be handed
/// to some unrelated program later.
pub fn started(pid: u32) -> Option<String> {
    let out = Command::new("ps").args(["-o", "lstart=", "-p", &pid.to_string()]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub fn is_running(pid: u32, since: &str) -> bool {
    started(pid).as_deref() == Some(since)
}

/// Send a signal (`TERM`, `KILL`) to a program and everything it started.
pub fn signal(pid: u32, name: &str) {
    let _ = Command::new("kill")
        .args(["-s", name, "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Note a program the kernel has just started.
pub fn track(conn: &Connection, pid: u32, proc: &str, kind: &str) {
    if let Some(since) = started(pid) {
        let _ = conn.execute(
            "INSERT OR REPLACE INTO children (pid, started, proc, kind) VALUES (?1, ?2, ?3, ?4)",
            params![pid, since, proc, kind],
        );
    }
}

/// Note that it has ended.
pub fn untrack(conn: &Connection, pid: u32) {
    let _ = conn.execute("DELETE FROM children WHERE pid = ?1", [pid]);
}

/// The tracked programs: all of them, or those of one process.
pub fn tracked(conn: &Connection, proc: Option<&str>) -> Vec<(u32, String)> {
    let Ok(mut stmt) = conn.prepare("SELECT pid, started FROM children WHERE ?1 IS NULL OR proc = ?1") else {
        return Vec::new();
    };
    stmt.query_map([proc], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// End a tracked program and everything it started: ask, wait a little, then
/// insist. Does nothing if the pid now belongs to something else.
pub fn end(pid: u32, since: &str) {
    if !is_running(pid, since) {
        return;
    }
    signal(pid, "TERM");
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline && is_running(pid, since) {
        thread::sleep(Duration::from_millis(100));
    }
    signal(pid, "KILL");
}

/// End several tracked programs at once and wait until they are gone.
pub fn end_all(programs: Vec<(u32, String)>) {
    let ending: Vec<_> = programs.into_iter().map(|(pid, since)| thread::spawn(move || end(pid, &since))).collect();
    for thread in ending {
        let _ = thread.join();
    }
}

/// End a program the kernel itself is the parent of, and everything it started.
pub fn end_child(child: &mut Child) {
    let pid = child.id();
    signal(pid, "TERM");
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
        thread::sleep(Duration::from_millis(50));
    }
    signal(pid, "KILL");
    let _ = child.wait();
}
