//! The registry: who exists, what they have said to each other, what happened.
//!
//! Code is not here. Each process's code lives on its own git branch. The
//! database holds only what is not code: the list of processes, their
//! mailboxes, the programs the kernel has running, and the log of facts.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type Res<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Changed whenever the tables change, so an old database is refused with a
/// clear message instead of failing in some odd way later.
const VERSION: i64 = 2;

const SCHEMA: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE processes (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE,
    home TEXT NOT NULL,              -- the jarvis home it currently runs in
    branch TEXT NOT NULL,
    harness TEXT NOT NULL,
    session TEXT,
    status TEXT NOT NULL,            -- 'alive' or 'stopped'
    busy INTEGER NOT NULL DEFAULT 0, -- a turn is in progress
    busy_since INTEGER,              -- when that turn began
    failures INTEGER NOT NULL DEFAULT 0, -- turns that failed in a row
    parent TEXT NOT NULL,
    created_at INTEGER NOT NULL);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    sender TEXT NOT NULL,
    recipient TEXT NOT NULL,
    body TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',  -- pending, taken, handled
    attempts INTEGER NOT NULL DEFAULT 0,     -- times it was given to the recipient
    created_at INTEGER NOT NULL);
CREATE INDEX messages_waiting ON messages (status, recipient, id);
-- Every program the kernel has started and not yet seen end. `started` is
-- the program's start time: a pid alone can be reused by something else.
CREATE TABLE children (
    pid INTEGER PRIMARY KEY,
    started TEXT NOT NULL,
    proc TEXT NOT NULL,
    kind TEXT NOT NULL);             -- 'run', 'handle' or 'agent'
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    proc TEXT NOT NULL,
    kind TEXT NOT NULL,
    data TEXT NOT NULL);
";

/// Addresses that are not processes. Mail to them is stored and never dispatched.
pub const HUMAN: &str = "human";
pub const KERNEL: &str = "kernel";

/// A jarvis installation: a git repository with a `.jarvis/` directory in it.
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    /// `$JARVIS_HOME`, or the nearest directory above the current one that
    /// holds `.jarvis/jarvis.db`.
    pub fn find() -> Res<Home> {
        if let Ok(root) = std::env::var("JARVIS_HOME") {
            return Ok(Home { root: PathBuf::from(root) });
        }
        let mut dir = std::env::current_dir()?;
        loop {
            if dir.join(".jarvis/jarvis.db").exists() {
                return Ok(Home { root: dir });
            }
            if !dir.pop() {
                return Err("not inside a jarvis repository (run `jarvis init` in one)".into());
            }
        }
    }

    pub fn dot(&self) -> PathBuf {
        self.root.join(".jarvis")
    }

    pub fn db_path(&self) -> PathBuf {
        self.dot().join("jarvis.db")
    }

    /// The working directory of a process: a checkout of its branch.
    pub fn proc_dir(&self, id: &str) -> PathBuf {
        self.dot().join("proc").join(id)
    }

    pub fn open(&self) -> Res<Connection> {
        open(&self.db_path())
    }
}

pub fn open(path: &Path) -> Res<Connection> {
    let conn = Connection::open(path)?;
    // WAL lets many `jarvis` commands and the kernel use the database at once.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(Duration::from_secs(15))?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version == VERSION {
        return Ok(conn);
    }
    let tables: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'", [], |r| r.get(0))?;
    if version != 0 || tables != 0 {
        return Err(format!(
            "{} was made by a different version of jarvis. Move the `.jarvis` directory aside and run `jarvis init` again.",
            path.display()
        )
        .into());
    }
    conn.execute_batch(&format!("BEGIN; {SCHEMA} PRAGMA user_version = {VERSION}; COMMIT;"))?;
    Ok(conn)
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
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Record a fact.
pub fn event(conn: &Connection, proc: &str, kind: &str, data: Value) -> Res<()> {
    conn.execute(
        "INSERT INTO events (at, proc, kind, data) VALUES (?1, ?2, ?3, ?4)",
        params![now_ms(), proc, kind, data.to_string()],
    )?;
    Ok(())
}

pub struct Proc {
    pub id: String,
    pub name: Option<String>,
    pub home: String,
    pub branch: String,
    pub harness: String,
    pub session: Option<String>,
    pub status: String,
    pub busy: bool,
    pub busy_since: Option<i64>,
    pub failures: i64,
    pub parent: String,
    pub created_at: i64,
}

impl Proc {
    /// What to call it when talking to people and agents.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{name} ({})", self.id),
            None => self.id.clone(),
        }
    }

    /// Its name, or its id if it has none.
    pub fn short(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

const PROC_COLUMNS: &str =
    "id, name, home, branch, harness, session, status, busy, busy_since, failures, parent, created_at";

fn proc_from_row(r: &rusqlite::Row) -> rusqlite::Result<Proc> {
    Ok(Proc {
        id: r.get(0)?,
        name: r.get(1)?,
        home: r.get(2)?,
        branch: r.get(3)?,
        harness: r.get(4)?,
        session: r.get(5)?,
        status: r.get(6)?,
        busy: r.get::<_, i64>(7)? != 0,
        busy_since: r.get(8)?,
        failures: r.get(9)?,
        parent: r.get(10)?,
        created_at: r.get(11)?,
    })
}

pub fn procs(conn: &Connection) -> Res<Vec<Proc>> {
    let mut stmt = conn.prepare(&format!("SELECT {PROC_COLUMNS} FROM processes ORDER BY created_at, id"))?;
    let rows = stmt.query_map([], proc_from_row)?.collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Find a process by id or by name.
pub fn find_proc(conn: &Connection, who: &str) -> Res<Option<Proc>> {
    Ok(conn
        .query_row(
            &format!("SELECT {PROC_COLUMNS} FROM processes WHERE id = ?1 OR name = ?1"),
            [who],
            proc_from_row,
        )
        .optional()?)
}

/// Turn what someone typed (an id, a name, `human`) into an address.
pub fn resolve(conn: &Connection, who: &str) -> Res<String> {
    if who == HUMAN || who == KERNEL {
        return Ok(who.to_string());
    }
    match find_proc(conn, who)? {
        Some(proc) => Ok(proc.id),
        None => Err(format!("no such process: {who} (see `jarvis ps`)").into()),
    }
}

/// Put a message in a mailbox.
pub fn post(conn: &Connection, sender: &str, recipient: &str, body: &str) -> Res<i64> {
    // Mail to a non-process is never dispatched, so it is not left pending.
    let status = if recipient == HUMAN || recipient == KERNEL { "handled" } else { "pending" };
    conn.execute(
        "INSERT INTO messages (sender, recipient, body, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![sender, recipient, body, status, now_ms()],
    )?;
    let id = conn.last_insert_rowid();
    event(conn, sender, "send", json!({"message": id, "to": recipient}))?;
    Ok(id)
}
