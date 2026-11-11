//! The programs the kernel starts: each process's `.jarvis/run`, its
//! `.jarvis/handle`, and its agent's harness.
//!
//! Two rules keep them under control:
//!
//! - Each is started as the leader of its own process group, so it and
//!   everything it starts in turn can be ended together.
//! - Each is noted in the kernel's database while it runs, with its start
//!   time. A kernel that starts after it was killed finds there what it left
//!   behind, and ends it before doing anything else.
//!
//! Signals and start times come straight from the operating system.

use rusqlite::{Connection, params};
use std::thread;
use std::time::{Duration, Instant};

/// How long a program gets to end by itself after being asked to.
const GRACE: Duration = Duration::from_secs(3);

/// When the program with this pid started, if there is one. A pid together
/// with its start time names one program for good; a pid alone may be handed
/// to some unrelated program later.
#[cfg(target_os = "macos")]
pub fn started(pid: u32) -> Option<String> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let got = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDTBSDINFO, 0, &mut info as *mut _ as *mut libc::c_void, size) };
    // A program that has ended but was not collected yet no longer counts.
    (got == size && info.pbi_status != libc::SZOMB).then(|| format!("{}.{}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(target_os = "linux")]
pub fn started(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The program's name, in brackets, may itself hold spaces and brackets.
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let state = fields.next()?;
    // A program that has ended but was not collected yet no longer counts.
    if state == "Z" || state == "X" {
        return None;
    }
    // The start time is the 22nd field of the line, the 20th after the name.
    fields.nth(18).map(str::to_string)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn started(_pid: u32) -> Option<String> {
    None
}

pub fn is_running(pid: u32, since: &str) -> bool {
    started(pid).as_deref() == Some(since)
}

/// Send a signal to a program and everything it started.
pub fn signal(pid: u32, signal: libc::c_int) {
    if pid > 1 {
        unsafe {
            libc::kill(-(pid as libc::pid_t), signal);
        }
    }
}

/// Note a program the kernel has just started.
pub fn track(conn: &Connection, pid: u32, proc: &str, kind: &str) {
    if let Some(since) = started(pid) {
        let _ = conn.execute("INSERT OR REPLACE INTO children (pid, started, proc, kind) VALUES (?1, ?2, ?3, ?4)", params![pid, since, proc, kind]);
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
    signal(pid, libc::SIGTERM);
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline && is_running(pid, since) {
        thread::sleep(Duration::from_millis(50));
    }
    signal(pid, libc::SIGKILL);
}

/// End several tracked programs at once and wait until they are gone.
pub fn end_all(programs: Vec<(u32, String)>) {
    let ending: Vec<_> = programs.into_iter().map(|(pid, since)| thread::spawn(move || end(pid, &since))).collect();
    for thread in ending {
        let _ = thread.join();
    }
}
