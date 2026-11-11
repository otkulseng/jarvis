//! Records, and the database built from them.
//!
//! Everything that happens is a *record*: a message sent, a turn finished, a
//! process created. A record is written once, by one kernel, and never
//! changed. It is named by that kernel and its number there (`laptop:412`),
//! so two copies of a record are always the same record, and records from
//! any number of kernels can be put together in any order.
//!
//! The database is what the records add up to: who exists, what state each
//! process is in, which messages wait. `apply` is the one place that decides
//! what a record means for it. A kernel applies its own records as it writes
//! them and other kernels' as it reads them from the store; a reader without
//! a kernel builds the same database from the store alone.
//!
//! A kernel's database also holds what only that kernel needs while it runs
//! (the programs it started, messages it holds for other kernels, its links).

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Names that are not processes. `human@K` is the person, as seen from
/// kernel K; `kernel@K` is the kernel itself. Mail to them is kept and never
/// handed to an agent.
pub const HUMAN: &str = "human";
pub const KERNEL: &str = "kernel";

/// Changed whenever the tables change, so an old database is refused with a
/// clear message instead of failing in some odd way later.
const VERSION: i64 = 3;

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- Every record this database has seen. `seq` is the order they were seen in
-- here. `n` is empty for what is kept here only (the steps of a turn).
CREATE TABLE records (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    k TEXT NOT NULL, n INTEGER, at INTEGER NOT NULL,
    kind TEXT NOT NULL, proc TEXT NOT NULL, data TEXT NOT NULL,
    UNIQUE (k, n));
CREATE INDEX records_by_proc ON records (proc, kind, seq);
CREATE TABLE kernels (
    name TEXT PRIMARY KEY, identity TEXT, created_at INTEGER,
    closed INTEGER NOT NULL DEFAULT 0, closed_by TEXT,
    last_at INTEGER NOT NULL DEFAULT 0, last_n INTEGER NOT NULL DEFAULT 0);
CREATE TABLE procs (
    addr TEXT PRIMARY KEY, name TEXT NOT NULL, kernel TEXT NOT NULL,
    branch TEXT NOT NULL, harness TEXT NOT NULL, session TEXT,
    status TEXT NOT NULL,                 -- alive, stopped, removed
    busy INTEGER NOT NULL DEFAULT 0, busy_since INTEGER,
    failures INTEGER NOT NULL DEFAULT 0,
    parent TEXT NOT NULL, from_what TEXT, created_at INTEGER NOT NULL);
CREATE TABLE tags (addr TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY (addr, key));
-- sn, rn: the names of sender and recipient; rk: the recipient's kernel.
CREATE TABLE messages (
    id TEXT PRIMARY KEY, seq INTEGER NOT NULL, at INTEGER NOT NULL,
    sender TEXT NOT NULL, recipient TEXT NOT NULL,
    sn TEXT NOT NULL, rn TEXT NOT NULL, rk TEXT NOT NULL,
    body TEXT NOT NULL, status TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0, turn INTEGER);
CREATE INDEX messages_waiting ON messages (recipient, status, seq);
CREATE INDEX messages_by_at ON messages (at, id);
CREATE INDEX messages_by_sn ON messages (sn, seq);
CREATE INDEX messages_by_rn ON messages (rn, seq);
-- Files of the store that were read already.
CREATE TABLE sources (path TEXT PRIMARY KEY);
-- What only the kernel that owns this database needs.
CREATE TABLE children (pid INTEGER PRIMARY KEY, started TEXT NOT NULL, proc TEXT NOT NULL, kind TEXT NOT NULL);
CREATE TABLE outbox (id TEXT PRIMARY KEY, dest TEXT NOT NULL, payload TEXT NOT NULL);
CREATE TABLE seen (id TEXT PRIMARY KEY);
CREATE TABLE links (spec TEXT PRIMARY KEY);
";

pub fn open(path: &Path) -> Res<Connection> {
    let conn = Connection::open(path)?;
    // WAL lets the kernel write while its readers read.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(Duration::from_secs(15))?;
    prepare(&conn, &path.display().to_string())?;
    Ok(conn)
}

/// A database that lives only as long as the command that made it.
pub fn open_memory() -> Res<Connection> {
    let conn = Connection::open_in_memory()?;
    prepare(&conn, "memory")?;
    Ok(conn)
}

fn prepare(conn: &Connection, what: &str) -> Res<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == VERSION {
        return Ok(());
    }
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))?;
    if version != 0 || tables != 0 {
        return Err(format!("{what} was made by a different version of jarvis. Move that kernel's directory aside, or start a kernel under a new name.").into());
    }
    conn.execute_batch(&format!("BEGIN; {SCHEMA} PRAGMA user_version = {VERSION}; COMMIT;"))?;
    Ok(())
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// `bytes` random bytes, as hexadecimal.
pub fn random_hex(bytes: usize) -> Res<String> {
    use std::io::Read;
    let mut buffer = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buffer)?;
    Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
}

pub fn get_meta(conn: &Connection, key: &str) -> Res<Option<String>> {
    Ok(conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Res<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

// -------------------------------------------------------------- addresses

/// `name@kernel`.
pub fn at(name: &str, kernel: &str) -> String {
    format!("{name}@{kernel}")
}

pub fn name_of(addr: &str) -> &str {
    addr.split_once('@').map_or(addr, |(name, _)| name)
}

pub fn kernel_of(addr: &str) -> &str {
    addr.split_once('@').map_or("", |(_, kernel)| kernel)
}

/// Whether mail to this address is only kept, never handed to an agent.
pub fn is_person(addr: &str) -> bool {
    matches!(name_of(addr), HUMAN | KERNEL)
}

/// What can be a process's or a kernel's name: it becomes part of a git
/// branch name and of a file name.
pub fn check_name(name: &str, what: &str) -> Res<()> {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    if name.is_empty() || !name.chars().all(plain) || name.starts_with(['-', '.']) || name.ends_with('.') || name.contains("..") {
        return Err(format!("`{name}` cannot be the name of a {what}: use letters, digits, `-`, `_` and `.`, starting with a letter or digit").into());
    }
    Ok(())
}

// ---------------------------------------------------------------- records

#[derive(Clone, Debug)]
pub struct Rec {
    pub k: String,
    /// Its number in its kernel's log; `None` for what is kept locally only.
    pub n: Option<i64>,
    pub at: i64,
    pub kind: String,
    /// Who it is about: an address.
    pub proc: String,
    pub data: Value,
}

impl Rec {
    pub fn to_value(&self) -> Value {
        json!({"k": self.k, "n": self.n, "at": self.at, "kind": self.kind, "proc": self.proc, "data": self.data})
    }

    pub fn from_value(v: &Value) -> Option<Rec> {
        Some(Rec {
            k: v["k"].as_str()?.to_string(),
            n: v["n"].as_i64(),
            at: v["at"].as_i64()?,
            kind: v["kind"].as_str()?.to_string(),
            proc: v["proc"].as_str().unwrap_or("").to_string(),
            data: v["data"].clone(),
        })
    }
}

/// Add a record. Returns where it landed in this database's order, or `None`
/// if it was here already (then nothing changes).
pub fn insert(conn: &Connection, r: &Rec) -> Res<Option<i64>> {
    let added = conn.prepare_cached("INSERT OR IGNORE INTO records (k, n, at, kind, proc, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?.execute(params![
        r.k,
        r.n,
        r.at,
        r.kind,
        r.proc,
        r.data.to_string()
    ])?;
    if added == 0 {
        return Ok(None);
    }
    let seq = conn.last_insert_rowid();
    apply(conn, r, seq)?;
    Ok(Some(seq))
}

fn ids(d: &Value, key: &str) -> Vec<String> {
    d[key].as_array().map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// What a record means for the database. The one place that decides.
fn apply(conn: &Connection, r: &Rec, seq: i64) -> Res<()> {
    let d = &r.data;
    let text = |key: &str| d[key].as_str().unwrap_or("");
    if let Some(n) = r.n {
        conn.prepare_cached(
            "INSERT INTO kernels (name, last_at, last_n) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET last_at = MAX(last_at, excluded.last_at), last_n = MAX(last_n, excluded.last_n)",
        )?
        .execute(params![r.k, r.at, n])?;
    }
    // A turn is over, however it ended: what it had not dealt with waits again.
    let turn_over = |failures: Option<i64>| -> Res<()> {
        conn.execute(
            "UPDATE procs SET busy = 0, busy_since = NULL, failures = COALESCE(?2, failures) WHERE addr = ?1",
            params![r.proc, failures],
        )?;
        conn.execute("UPDATE messages SET status = 'pending' WHERE recipient = ?1 AND status = 'taken'", [&r.proc])?;
        Ok(())
    };
    let handled = |key: &str| -> Res<()> {
        for id in ids(d, key) {
            conn.execute("UPDATE messages SET status = 'handled' WHERE id = ?1", [id])?;
        }
        Ok(())
    };
    match r.kind.as_str() {
        "kernel" => {
            conn.execute("UPDATE kernels SET identity = ?2, created_at = ?3 WHERE name = ?1", params![r.k, text("identity"), r.at])?;
        }
        "up" => {
            // Whatever it was in the middle of when it last stopped is over.
            conn.execute("UPDATE kernels SET closed = 0, closed_by = NULL WHERE name = ?1", [&r.k])?;
            conn.execute("UPDATE procs SET busy = 0, busy_since = NULL, failures = 0 WHERE kernel = ?1", [&r.k])?;
            conn.execute("UPDATE messages SET status = 'pending' WHERE rk = ?1 AND status = 'taken'", [&r.k])?;
        }
        "closed" => {
            let which = if text("kernel").is_empty() { r.k.as_str() } else { text("kernel") };
            conn.execute("INSERT OR IGNORE INTO kernels (name) VALUES (?1)", [which])?;
            conn.execute("UPDATE kernels SET closed = 1, closed_by = ?2 WHERE name = ?1", params![which, text("by")])?;
            conn.execute("UPDATE procs SET busy = 0, busy_since = NULL WHERE kernel = ?1", [which])?;
        }
        "spawn" => {
            conn.execute(
                "INSERT INTO procs (addr, name, kernel, branch, harness, status, parent, from_what, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'alive', ?6, ?7, ?8)
                 ON CONFLICT(addr) DO UPDATE SET branch = excluded.branch, harness = excluded.harness, status = 'alive',
                     session = NULL, busy = 0, busy_since = NULL, failures = 0, parent = excluded.parent,
                     from_what = excluded.from_what, created_at = excluded.created_at",
                params![r.proc, name_of(&r.proc), kernel_of(&r.proc), text("branch"), text("harness"), text("parent"), d["from"].as_str(), r.at],
            )?;
            conn.execute("DELETE FROM tags WHERE addr = ?1", [&r.proc])?;
            set_tags(conn, &r.proc, &d["tags"])?;
        }
        "stop" => {
            conn.execute("UPDATE procs SET status = 'stopped' WHERE addr = ?1 AND status != 'removed'", [&r.proc])?;
        }
        "start" => {
            conn.execute("UPDATE procs SET status = 'alive', failures = 0 WHERE addr = ?1 AND status != 'removed'", [&r.proc])?;
        }
        "remove" => {
            conn.execute("UPDATE procs SET status = 'removed', busy = 0, busy_since = NULL WHERE addr = ?1", [&r.proc])?;
            conn.execute("DELETE FROM tags WHERE addr = ?1", [&r.proc])?;
            conn.execute("UPDATE messages SET status = 'dropped' WHERE recipient = ?1 AND status IN ('pending', 'taken')", [&r.proc])?;
        }
        "tag" => set_tags(conn, &r.proc, &d["set"])?,
        "send" | "arrive" => {
            let (to, id) = (text("to"), text("id"));
            if id.is_empty() || to.is_empty() {
                return Ok(());
            }
            // It has arrived when the kernel that wrote this is the recipient's.
            let here = kernel_of(to) == r.k;
            let status = match (here, is_person(to)) {
                (true, true) => "handled",
                (true, false) => "pending",
                (false, _) => "sent",
            };
            conn.prepare_cached(
                "INSERT INTO messages (id, seq, at, sender, recipient, sn, rn, rk, body, status, turn)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(id) DO UPDATE SET status = excluded.status
                     WHERE messages.status IN ('sent', 'forwarded') AND excluded.status != 'sent'",
            )?
            .execute(params![
                id,
                seq,
                d["at"].as_i64().unwrap_or(r.at),
                text("from"),
                to,
                name_of(text("from")),
                name_of(to),
                kernel_of(to),
                text("body"),
                status,
                d["turn"].as_i64()
            ])?;
        }
        "forward" => {
            conn.execute("UPDATE messages SET status = 'forwarded' WHERE id = ?1 AND status = 'sent'", [text("message")])?;
        }
        "undeliverable" => {
            conn.execute(
                "UPDATE messages SET status = 'undeliverable' WHERE id = ?1 AND status IN ('sent', 'forwarded', 'pending')",
                [text("message")],
            )?;
        }
        "turn_start" => {
            conn.execute("UPDATE procs SET busy = 1, busy_since = ?2 WHERE addr = ?1", params![r.proc, r.at])?;
            for id in ids(d, "messages") {
                conn.execute(
                    "UPDATE messages SET status = 'taken', attempts = attempts + 1 WHERE id = ?1 AND status IN ('sent', 'forwarded', 'pending')",
                    [id],
                )?;
            }
        }
        "handled_by_code" => handled("messages")?,
        "turn" => {
            handled("messages")?;
            if let Some(session) = d["session"].as_str() {
                conn.execute("UPDATE procs SET session = ?2 WHERE addr = ?1", params![r.proc, session])?;
            }
            turn_over(Some(0))?;
        }
        "turn_failed" => turn_over(Some(d["failures"].as_i64().unwrap_or(1)))?,
        "turn_interrupted" | "turn_end" => turn_over(None)?,
        _ => {}
    }
    Ok(())
}

fn set_tags(conn: &Connection, addr: &str, tags: &Value) -> Res<()> {
    let Some(tags) = tags.as_object() else { return Ok(()) };
    for (key, value) in tags {
        let value = value.as_str().unwrap_or("");
        if value.is_empty() {
            conn.execute("DELETE FROM tags WHERE addr = ?1 AND key = ?2", params![addr, key])?;
        } else {
            conn.execute(
                "INSERT INTO tags (addr, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(addr, key) DO UPDATE SET value = excluded.value",
                params![addr, key, value],
            )?;
        }
    }
    Ok(())
}

// -------------------------------------------------------------- processes

pub struct Proc {
    pub addr: String,
    pub name: String,
    pub kernel: String,
    pub branch: String,
    pub harness: String,
    pub session: Option<String>,
    pub status: String,
    pub busy: bool,
    pub busy_since: Option<i64>,
    pub failures: i64,
    pub parent: String,
    pub from: Option<String>,
    pub created_at: i64,
}

const PROC_COLUMNS: &str = "addr, name, kernel, branch, harness, session, status, busy, busy_since, failures, parent, from_what, created_at";

fn proc_from_row(r: &rusqlite::Row) -> rusqlite::Result<Proc> {
    Ok(Proc {
        addr: r.get(0)?,
        name: r.get(1)?,
        kernel: r.get(2)?,
        branch: r.get(3)?,
        harness: r.get(4)?,
        session: r.get(5)?,
        status: r.get(6)?,
        busy: r.get::<_, i64>(7)? != 0,
        busy_since: r.get(8)?,
        failures: r.get(9)?,
        parent: r.get(10)?,
        from: r.get(11)?,
        created_at: r.get(12)?,
    })
}

/// Every process that was not removed, on kernels that have not closed
/// (`all`: on closed ones too).
pub fn procs(conn: &Connection, all: bool) -> Res<Vec<Proc>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {PROC_COLUMNS} FROM procs WHERE status != 'removed'
           AND (?1 OR NOT COALESCE((SELECT closed FROM kernels WHERE name = procs.kernel), 0))
         ORDER BY created_at, addr"
    ))?;
    let rows = stmt.query_map([all], proc_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// The process with this address, if it exists and was not removed.
pub fn find_proc(conn: &Connection, addr: &str) -> Res<Option<Proc>> {
    Ok(conn
        .prepare_cached(&format!("SELECT {PROC_COLUMNS} FROM procs WHERE addr = ?1 AND status != 'removed'"))?
        .query_row([addr], proc_from_row)
        .optional()?)
}

/// The processes called `name`, on any kernel that has not closed.
pub fn procs_named(conn: &Connection, name: &str) -> Res<Vec<Proc>> {
    Ok(procs(conn, false)?.into_iter().filter(|p| p.name == name).collect())
}

pub fn kernel_closed(conn: &Connection, name: &str) -> bool {
    conn.query_row("SELECT closed FROM kernels WHERE name = ?1", [name], |r| r.get::<_, i64>(0)).map(|c| c != 0).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(k: &str, n: i64, kind: &str, proc: &str, data: Value) -> Rec {
        Rec { k: k.into(), n: Some(n), at: n, kind: kind.into(), proc: proc.into(), data }
    }

    fn status(conn: &Connection, id: &str) -> String {
        conn.query_row("SELECT status FROM messages WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    #[test]
    fn a_record_counts_once() {
        let conn = open_memory().unwrap();
        let r = rec("a", 1, "send", "x@a", json!({"id": "a:1", "from": "x@a", "to": "y@a", "body": "hi"}));
        assert!(insert(&conn, &r).unwrap().is_some());
        assert!(insert(&conn, &r).unwrap().is_none());
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn a_message_between_kernels_whatever_order_the_logs_are_read_in() {
        let send = rec("a", 1, "send", "x@a", json!({"id": "a:1", "from": "x@a", "to": "y@b", "body": "hi"}));
        let forward = rec("a", 2, "forward", "x@a", json!({"message": "a:1", "next": "b"}));
        let arrive = rec("b", 1, "arrive", "y@b", json!({"id": "a:1", "from": "x@a", "to": "y@b", "body": "hi"}));
        let start = rec("b", 2, "turn_start", "y@b", json!({"messages": ["a:1"]}));
        let turn = rec("b", 3, "turn", "y@b", json!({"messages": ["a:1"], "reply": "ok"}));

        let sender_first = open_memory().unwrap();
        insert(&sender_first, &send).unwrap();
        assert_eq!(status(&sender_first, "a:1"), "sent");
        insert(&sender_first, &forward).unwrap();
        assert_eq!(status(&sender_first, "a:1"), "forwarded");
        for r in [&arrive, &start, &turn] {
            insert(&sender_first, r).unwrap();
        }
        assert_eq!(status(&sender_first, "a:1"), "handled");

        let recipient_first = open_memory().unwrap();
        for r in [&arrive, &start, &turn, &send, &forward] {
            insert(&recipient_first, r).unwrap();
        }
        assert_eq!(status(&recipient_first, "a:1"), "handled");
    }

    #[test]
    fn a_turn_that_fails_puts_its_mail_back() {
        let conn = open_memory().unwrap();
        insert(&conn, &rec("a", 1, "spawn", "y@a", json!({"branch": "y@a", "harness": "h", "parent": "human@a"}))).unwrap();
        insert(&conn, &rec("a", 2, "send", "human@a", json!({"id": "a:2", "from": "human@a", "to": "y@a", "body": "hi"}))).unwrap();
        insert(&conn, &rec("a", 3, "turn_start", "y@a", json!({"messages": ["a:2"]}))).unwrap();
        assert_eq!(status(&conn, "a:2"), "taken");
        assert!(find_proc(&conn, "y@a").unwrap().unwrap().busy);
        insert(&conn, &rec("a", 4, "turn_failed", "y@a", json!({"messages": ["a:2"], "failures": 1}))).unwrap();
        assert_eq!(status(&conn, "a:2"), "pending");
        let p = find_proc(&conn, "y@a").unwrap().unwrap();
        assert!(!p.busy);
        assert_eq!(p.failures, 1);
        // A kernel that comes up again was not in the middle of anything.
        insert(&conn, &rec("a", 5, "turn_start", "y@a", json!({"messages": ["a:2"]}))).unwrap();
        insert(&conn, &rec("a", 6, "up", "kernel@a", json!({}))).unwrap();
        assert_eq!(status(&conn, "a:2"), "pending");
    }

    #[test]
    fn removing_keeps_the_history() {
        let conn = open_memory().unwrap();
        insert(&conn, &rec("a", 1, "spawn", "y@a", json!({"branch": "y@a", "harness": "h", "parent": "human@a", "tags": {"project": "moe"}}))).unwrap();
        insert(&conn, &rec("a", 2, "send", "human@a", json!({"id": "a:2", "from": "human@a", "to": "y@a", "body": "hi"}))).unwrap();
        insert(&conn, &rec("a", 3, "remove", "y@a", json!({"by": "human@a"}))).unwrap();
        assert!(find_proc(&conn, "y@a").unwrap().is_none());
        assert_eq!(status(&conn, "a:2"), "dropped");
        let records: i64 = conn.query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0)).unwrap();
        assert_eq!(records, 3);
    }
}
