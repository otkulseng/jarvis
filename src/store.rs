//! The store: where records are kept for good.
//!
//!     STORE/
//!       <kernel>/
//!         kernel.json                  who this kernel is; written once
//!         records-000000000212.db      its records 1 to 212, in one SQLite file
//!         log/000000000213-000000000240.jsonl
//!                                      its records 213 to 240, one per line
//!
//! Everything of a kernel is in that kernel's directory, and nothing in a
//! store belongs to more than one kernel. A kernel numbers its records 1, 2,
//! 3, ... without gaps, so the name of a file says exactly what is in it.
//! That is what makes the store safe for anyone to read, copy and compact at
//! any time, with no locks and nobody asked:
//!
//! - A file is written once and never changed. It appears under its name
//!   only when it is complete.
//! - Two files of the same name hold the same records, whoever wrote them.
//! - `records-N.db` holds everything up to N. So a log file that ends at or
//!   below N, and a `records-M.db` with M below N, are no longer needed and
//!   may be deleted by anyone. Nothing else is ever deleted.
//!
//! `compact` (below) follows from these: write `records-N.db`, publish it
//! unless it is there already, delete what it covers.

use crate::index::{self, Rec, Res};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};

pub struct Store {
    pub dir: PathBuf,
    /// Per kernel, the number of its last record read from the store (also
    /// noted in the database, so it survives a restart).
    read_to: HashMap<String, i64>,
    /// Kernels whose `kernel.json` was read.
    known: HashSet<String>,
}

/// Write a file so that it appears complete or not at all.
fn write_whole(path: &Path, bytes: &[u8]) -> Res<()> {
    let dir = path.parent().ok_or("a file needs a directory")?;
    fs::create_dir_all(dir)?;
    let beside = dir.join(format!(".tmp-{}-{}", std::process::id(), index::random_hex(4)?));
    let mut file = fs::File::create(&beside)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&beside, path)?;
    Ok(())
}

/// The names in a directory that are not hidden, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| !n.starts_with('.')).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// The first and last record number a log file holds, from its name.
fn range_of(segment: &str) -> Option<(i64, i64)> {
    let (first, last) = segment.strip_suffix(".jsonl")?.split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?))
}

/// The last record number a records file holds, from its name.
fn upto_of(name: &str) -> Option<i64> {
    name.strip_prefix("records-")?.strip_suffix(".db")?.parse().ok()
}

fn records_name(upto: i64) -> String {
    format!("records-{upto:012}.db")
}

/// What one kernel has in a store, from the names of its files alone.
struct Listing {
    /// The highest `records-N.db`: its N, or 0 if there is none.
    upto: i64,
    /// Every records file: (N, path).
    files: Vec<(i64, PathBuf)>,
    /// Every log file, in order: (first, last, path).
    segments: Vec<(i64, i64, PathBuf)>,
}

fn list(home: &Path) -> Listing {
    let files: Vec<(i64, PathBuf)> = names(home).into_iter().filter_map(|n| Some((upto_of(&n)?, home.join(n)))).collect();
    let log = home.join("log");
    let mut segments: Vec<(i64, i64, PathBuf)> = names(&log)
        .into_iter()
        .filter_map(|n| {
            let (first, last) = range_of(&n)?;
            Some((first, last, log.join(n)))
        })
        .collect();
    segments.sort();
    Listing { upto: files.iter().map(|(n, _)| *n).max().unwrap_or(0), files, segments }
}

impl Listing {
    fn records_file(&self) -> Option<&PathBuf> {
        self.files.iter().find(|(n, _)| *n == self.upto).map(|(_, path)| path)
    }

    /// The log files that carry on from record `from` without a gap, and the
    /// number of the last record they reach.
    fn following(&self, from: i64) -> (Vec<&(i64, i64, PathBuf)>, i64) {
        let (mut reached, mut files) = (from, Vec::new());
        for segment in &self.segments {
            if segment.1 <= reached {
                continue;
            }
            if segment.0 > reached + 1 {
                break;
            }
            reached = segment.1;
            files.push(segment);
        }
        (files, reached)
    }
}

fn open_records(path: &Path) -> Res<Connection> {
    // `immutable`: nobody changes this file, so no locks are taken on it.
    // That is what makes reading it safe on network storage.
    let uri = format!("file:{}?immutable=1", path.display().to_string().replace('?', "%3f").replace('#', "%23"));
    Ok(Connection::open_with_flags(uri, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)?)
}

/// The records of a log file, in order.
fn read_segment(path: &Path) -> Res<Vec<Rec>> {
    let mut records = Vec::new();
    for line in BufReader::new(fs::File::open(path)?).lines() {
        let Ok(value) = serde_json::from_str::<Value>(&line?) else { continue };
        let Some(rec) = Rec::from_value(&value) else { continue };
        if rec.n.is_some() {
            records.push(rec);
        }
    }
    Ok(records)
}

impl Store {
    pub fn open(dir: &Path, _conn: &Connection) -> Res<Store> {
        fs::create_dir_all(dir).map_err(|e| format!("could not create the store at {}: {e}", dir.display()))?;
        Ok(Store { dir: dir.to_path_buf(), read_to: HashMap::new(), known: HashSet::new() })
    }

    /// Say who a kernel is, once. Refuses if the store already knows another
    /// kernel by that name.
    pub fn announce(&self, name: &str, identity: &str, created_at: i64) -> Res<()> {
        let path = self.dir.join(name).join("kernel.json");
        if let Ok(text) = fs::read_to_string(&path) {
            let known: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if known["identity"] != identity {
                return Err(format!(
                    "the store at {} already holds the records of another kernel called `{name}`. Choose a name that was never used.",
                    self.dir.display()
                )
                .into());
            }
            return Ok(());
        }
        write_whole(&path, format!("{}\n", json!({"name": name, "identity": identity, "created_at": created_at})).as_bytes())
    }

    /// Write the kernel's records that are not in the store yet, as one new
    /// log file. Returns how many there were.
    pub fn ship(&self, conn: &Connection, me: &str) -> Res<usize> {
        let shipped: i64 = index::get_meta(conn, "shipped")?.and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut stmt = conn.prepare_cached("SELECT n, at, kind, proc, data FROM records WHERE k = ?1 AND n > ?2 ORDER BY n")?;
        let mut text = String::new();
        let (mut last, mut count) = (shipped, 0);
        let rows = stmt.query_map(params![me, shipped], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
        })?;
        for row in rows {
            let (n, at, kind, proc, data) = row?;
            last = n;
            count += 1;
            let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            text.push_str(&Rec { k: me.to_string(), n: Some(n), at, kind, proc, data }.to_value().to_string());
            text.push('\n');
        }
        if count == 0 {
            return Ok(0);
        }
        // Named by what it covers, so that the files follow each other without a gap.
        write_whole(&self.dir.join(me).join("log").join(format!("{:012}-{last:012}.jsonl", shipped + 1)), text.as_bytes())?;
        index::set_meta(conn, "shipped", &last.to_string())?;
        Ok(count)
    }

    /// Read what is new in the store into the database. `me`: the kernel
    /// whose database this is; its own files hold nothing it does not have.
    /// Returns how many records were new.
    pub fn ingest(&mut self, conn: &Connection, me: Option<&str>) -> Res<usize> {
        // In one go, which is much faster than record by record. What was
        // read before a failure is kept: it is true whatever came after.
        conn.execute_batch("BEGIN")?;
        let new = self.ingest_all(conn, me);
        conn.execute_batch("COMMIT")?;
        new
    }

    fn ingest_all(&mut self, conn: &Connection, me: Option<&str>) -> Res<usize> {
        let mut new = 0;
        for kernel in names(&self.dir) {
            if Some(kernel.as_str()) == me {
                continue;
            }
            let home = self.dir.join(&kernel);
            if !self.known.contains(&kernel) {
                if let Ok(text) = fs::read_to_string(home.join("kernel.json")) {
                    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                    know_kernel(conn, &kernel, v["identity"].as_str(), v["created_at"].as_i64())?;
                    self.known.insert(kernel.clone());
                }
            }
            let before = match self.read_to.get(&kernel) {
                Some(n) => *n,
                None => index::get_meta(conn, &format!("read:{kernel}"))?.and_then(|v| v.parse().ok()).unwrap_or(0),
            };
            let (count, reached) = match read_on(conn, &home, before) {
                Ok(read) => read,
                // A file that was listed and then gone was replaced by one
                // that covers it. Next time round, that one is read.
                Err(e) => {
                    if !is_gone(&e) {
                        eprintln!("jarvis: could not read the records of `{kernel}` from the store: {e}");
                    }
                    (0, before)
                }
            };
            if reached != before {
                index::set_meta(conn, &format!("read:{kernel}"), &reached.to_string())?;
            }
            self.read_to.insert(kernel, reached);
            new += count;
        }
        Ok(new)
    }
}

fn is_gone(e: &Box<dyn std::error::Error + Send + Sync>) -> bool {
    e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == ErrorKind::NotFound)
}

/// Read one kernel's records after number `from`. Returns how many were new
/// to the database, and the number of the last one read.
fn read_on(conn: &Connection, home: &Path, from: i64) -> Res<(usize, i64)> {
    let listing = list(home);
    let (mut count, mut reached) = (0, from);
    if listing.upto > reached {
        let path = listing.records_file().expect("the highest one is listed");
        if !path.exists() {
            return Err(Box::new(std::io::Error::from(ErrorKind::NotFound)));
        }
        let file = open_records(path)?;
        let mut stmt = file.prepare("SELECT k, n, at, kind, proc, data FROM records WHERE n > ?1 ORDER BY n")?;
        let rows = stmt.query_map([reached], |r| {
            Ok(Rec {
                k: r.get(0)?,
                n: r.get(1)?,
                at: r.get(2)?,
                kind: r.get(3)?,
                proc: r.get(4)?,
                data: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or(Value::Null),
            })
        })?;
        for rec in rows {
            if index::insert(conn, &rec?)?.is_some() {
                count += 1;
            }
        }
        reached = listing.upto;
    }
    for (_, last, path) in listing.following(reached).0 {
        for rec in read_segment(path)? {
            if rec.n > Some(reached) && index::insert(conn, &rec)?.is_some() {
                count += 1;
            }
        }
        reached = *last;
    }
    Ok((count, reached))
}

fn know_kernel(conn: &Connection, name: &str, identity: Option<&str>, created_at: Option<i64>) -> Res<()> {
    conn.execute(
        "INSERT INTO kernels (name, identity, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET identity = COALESCE(kernels.identity, excluded.identity),
             created_at = COALESCE(kernels.created_at, excluded.created_at)",
        params![name, identity, created_at],
    )?;
    Ok(())
}

/// Put one kernel's records into one file, `records-N.db`, and delete what
/// that file covers. Only that kernel's directory is touched.
///
/// Anyone may run this at any time, also several at once, with nothing but
/// the files to go by:
///
/// 1. From the names: the highest `records-M.db`, and the log files that
///    carry on from M without a gap, up to record N.
/// 2. Write records 1 to N into a file beside them.
/// 3. Give it the name `records-N.db`, unless a file of that name is there
///    already: then someone else wrote the same records, and this one is
///    thrown away.
/// 4. Delete every log file that ends at or below the highest N there is,
///    and every `records-M.db` below it.
///
/// A file is deleted only when a complete file that covers it has its name,
/// so every record is in some file at every moment.
pub fn compact(dir: &Path, kernel: &str) -> Res<String> {
    let home = dir.join(kernel);
    if !home.is_dir() {
        return Err(format!("the store {} has nothing of a kernel called `{kernel}`", dir.display()).into());
    }
    let mut said = format!("{kernel}: nothing to compact");
    // Tried again when a file it listed is gone: someone else compacted
    // meanwhile, and the files say how far they got.
    for attempt in 0.. {
        let listing = list(&home);
        let (segments, upto) = listing.following(listing.upto);
        if upto == listing.upto {
            break;
        }
        let paths: Vec<&PathBuf> = segments.iter().map(|s| &s.2).collect();
        match write_records(&home, kernel, listing.records_file(), &paths, upto) {
            Ok(count) => {
                said = format!("{kernel}/{}: {count} records", records_name(upto));
                break;
            }
            Err(e) if is_gone(&e) && attempt < 5 => continue,
            Err(e) => return Err(format!("could not write the records file for `{kernel}`, so nothing was changed: {e}").into()),
        }
    }
    tidy(&home);
    Ok(said)
}

/// Steps 2 and 3. Returns how many records the file holds.
fn write_records(home: &Path, kernel: &str, earlier: Option<&PathBuf>, segments: &[&PathBuf], upto: i64) -> Res<usize> {
    let beside = home.join(format!(".tmp-{}-{}.db", std::process::id(), index::random_hex(4)?));
    let build = || -> Res<usize> {
        let file = Connection::open(&beside)?;
        file.execute_batch(
            "CREATE TABLE records (k TEXT NOT NULL, n INTEGER NOT NULL, at INTEGER NOT NULL, kind TEXT NOT NULL,
                                   proc TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (k, n)) WITHOUT ROWID;
             BEGIN;",
        )?;
        let mut count = 0;
        let mut add = |k: &str, n: i64, at: i64, kind: &str, proc: &str, data: &str| -> Res<()> {
            if k == kernel && n <= upto {
                count += file.prepare_cached("INSERT OR IGNORE INTO records (k, n, at, kind, proc, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?.execute(params![k, n, at, kind, proc, data])?;
            }
            Ok(())
        };
        if let Some(path) = earlier {
            if !path.exists() {
                return Err(Box::new(std::io::Error::from(ErrorKind::NotFound)));
            }
            let old = open_records(path)?;
            let mut stmt = old.prepare("SELECT k, n, at, kind, proc, data FROM records")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                add(&r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, &r.get::<_, String>(3)?, &r.get::<_, String>(4)?, &r.get::<_, String>(5)?)?;
            }
        }
        for path in segments {
            for rec in read_segment(path)? {
                add(&rec.k, rec.n.expect("only numbered records are read"), rec.at, &rec.kind, &rec.proc, &rec.data.to_string())?;
            }
        }
        file.execute_batch("COMMIT;")?;
        Ok(count)
    };
    let publish = |count: usize| -> Res<usize> {
        let name = home.join(records_name(upto));
        // A hard link gives the file its name only if nothing has it yet.
        match fs::hard_link(&beside, &name) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            // Where links cannot be made: taking the name over is as good,
            // because a file of this name can only hold these records.
            Err(_) => fs::rename(&beside, &name)?,
        }
        Ok(count)
    };
    let done = build().and_then(publish);
    let _ = fs::remove_file(&beside);
    done
}

/// Step 4: delete what the highest records file covers. Says nothing and
/// fails at nothing: whatever is left is deleted by the next one to come by.
fn tidy(home: &Path) {
    let listing = list(home);
    for (upto, path) in &listing.files {
        if *upto < listing.upto {
            let _ = fs::remove_file(path);
        }
    }
    for (_, last, path) in &listing.segments {
        if *last <= listing.upto {
            let _ = fs::remove_file(path);
        }
    }
}

/// Copy into this store what another store has and this one lacks.
pub fn merge(dir: &Path, from: &Path) -> Res<String> {
    if !from.is_dir() {
        return Err(format!("{} is not a directory", from.display()).into());
    }
    fs::create_dir_all(dir)?;
    let (mut copied, mut refused) = (0usize, Vec::new());
    let mut copy = |source: &Path, target: &Path| -> Res<()> {
        if !target.exists() {
            write_whole(target, &fs::read(source)?)?;
            copied += 1;
        }
        Ok(())
    };
    for kernel in names(from) {
        let (theirs, ours) = (from.join(&kernel), dir.join(&kernel));
        if !theirs.is_dir() {
            continue;
        }
        let identity = |home: &Path| -> Option<Value> {
            let text = fs::read_to_string(home.join("kernel.json")).ok()?;
            Some(serde_json::from_str::<Value>(&text).ok()?["identity"].clone())
        };
        // Two different kernels that were given the same name cannot share a directory.
        if let (Some(theirs), Some(ours)) = (identity(&theirs), identity(&ours)) {
            if theirs != ours {
                refused.push(kernel);
                continue;
            }
        }
        if theirs.join("kernel.json").exists() {
            copy(&theirs.join("kernel.json"), &ours.join("kernel.json"))?;
        }
        // Only what is not covered here already.
        let (have, listing) = (list(&ours).upto, list(&theirs));
        if listing.upto > have {
            let path = listing.records_file().expect("the highest one is listed");
            copy(path, &ours.join(records_name(listing.upto)))?;
        }
        let have = have.max(listing.upto);
        for (_, last, path) in &listing.segments {
            if *last > have {
                copy(path, &ours.join("log").join(path.file_name().expect("a log file has a name")))?;
            }
        }
    }
    let mut said = format!("copied {copied} file(s) from {}", from.display());
    if !refused.is_empty() {
        said.push_str(&format!(
            "\nnot copied, because this store holds a different kernel of the same name: {}",
            refused.join(", ")
        ));
    }
    Ok(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jarvis-test-{name}-{}", index::random_hex(4).unwrap()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A kernel's database with `count` messages written by kernel `k`.
    fn kernel_with(k: &str, count: i64) -> Connection {
        let conn = index::open_memory().unwrap();
        for n in 1..=count {
            let id = format!("{k}:{n}");
            let rec = Rec {
                k: k.into(),
                n: Some(n),
                at: n,
                kind: "send".into(),
                proc: format!("x@{k}"),
                data: json!({"id": id, "from": format!("x@{k}"), "to": format!("y@{k}"), "body": format!("message {n}")}),
            };
            index::insert(&conn, &rec).unwrap();
        }
        conn
    }

    fn messages(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn shipped_records_are_read_by_others_once() {
        let dir = scratch("ship");
        let a = kernel_with("a", 5);
        let store = Store::open(&dir, &a).unwrap();
        store.announce("a", "id-a", 1).unwrap();
        assert_eq!(store.ship(&a, "a").unwrap(), 5);
        assert_eq!(store.ship(&a, "a").unwrap(), 0);

        let b = index::open_memory().unwrap();
        let mut reader = Store::open(&dir, &b).unwrap();
        assert_eq!(reader.ingest(&b, Some("b")).unwrap(), 5);
        assert_eq!(reader.ingest(&b, Some("b")).unwrap(), 0);
        assert_eq!(messages(&b), 5);
        // Another kernel may not take the name.
        assert!(store.announce("a", "someone-else", 2).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compacting_and_merging_lose_nothing() {
        let (one, two) = (scratch("one"), scratch("two"));
        let a = kernel_with("a", 40);
        let store = Store::open(&one, &a).unwrap();
        store.announce("a", "id-a", 1).unwrap();
        store.ship(&a, "a").unwrap();
        let b = kernel_with("b", 30);
        let other = Store::open(&two, &b).unwrap();
        other.announce("b", "id-b", 1).unwrap();
        other.ship(&b, "b").unwrap();

        // A reader that saw the files before they were compacted, and one after.
        let early = index::open_memory().unwrap();
        let mut early_reader = Store::open(&one, &early).unwrap();
        assert_eq!(early_reader.ingest(&early, None).unwrap(), 40);

        compact(&one, "a").unwrap();
        assert!(names(&one.join("a").join("log")).is_empty());
        assert!(one.join("a").join(records_name(40)).exists());
        merge(&one, &two).unwrap();
        // Compacting one kernel leaves the other's files as they were.
        compact(&one, "a").unwrap();
        assert_eq!(names(&one.join("b").join("log")).len(), 1);
        compact(&one, "b").unwrap();
        assert!(one.join("b").join(records_name(30)).exists());

        let late = index::open_memory().unwrap();
        Store::open(&one, &late).unwrap().ingest(&late, None).unwrap();
        assert_eq!(messages(&late), 70);
        assert_eq!(early_reader.ingest(&early, None).unwrap(), 30);
        assert_eq!(messages(&early), 70);

        // A compacted store merges like any other.
        let three = scratch("three");
        merge(&three, &one).unwrap();
        let copy = index::open_memory().unwrap();
        Store::open(&three, &copy).unwrap().ingest(&copy, None).unwrap();
        assert_eq!(messages(&copy), 70);
        for dir in [one, two, three] {
            fs::remove_dir_all(dir).unwrap();
        }
    }

    /// Many compactions of one kernel at once, while it keeps writing: every
    /// record stays readable, and one file is left.
    #[test]
    fn compacting_at_the_same_time_loses_nothing() {
        let dir = scratch("race");
        let a = index::open_memory().unwrap();
        let store = Store::open(&dir, &a).unwrap();
        store.announce("a", "id-a", 1).unwrap();
        let mut n = 0;
        for round in 0..30 {
            for _ in 0..7 {
                n += 1;
                let rec = Rec {
                    k: "a".into(),
                    n: Some(n),
                    at: n,
                    kind: "send".into(),
                    proc: "x@a".into(),
                    data: json!({"id": format!("a:{n}"), "from": "x@a", "to": "y@a", "body": "hello"}),
                };
                index::insert(&a, &rec).unwrap();
            }
            store.ship(&a, "a").unwrap();
            if round % 3 == 0 {
                let racers: Vec<_> = (0..4)
                    .map(|_| {
                        let dir = dir.clone();
                        std::thread::spawn(move || compact(&dir, "a").unwrap())
                    })
                    .collect();
                // Someone reads while they are at it.
                let reader = index::open_memory().unwrap();
                Store::open(&dir, &reader).unwrap().ingest(&reader, None).unwrap();
                for racer in racers {
                    racer.join().unwrap();
                }
                let reader = index::open_memory().unwrap();
                Store::open(&dir, &reader).unwrap().ingest(&reader, None).unwrap();
                assert_eq!(messages(&reader), n);
            }
        }
        compact(&dir, "a").unwrap();
        assert_eq!(names(&dir.join("a")), vec!["kernel.json".to_string(), "log".to_string(), records_name(n)]);
        assert!(names(&dir.join("a").join("log")).is_empty());
        let reader = index::open_memory().unwrap();
        Store::open(&dir, &reader).unwrap().ingest(&reader, None).unwrap();
        assert_eq!(messages(&reader), n);
        fs::remove_dir_all(dir).unwrap();
    }
}
