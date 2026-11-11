//! The store: where records are kept for good.
//!
//!     STORE/
//!       <kernel>/
//!         kernel.json                  who this kernel is; written once
//!         log/000000000001-000000000212.jsonl
//!                                      its records 1 to 212, one per line
//!       packs/<time>-<random>.db       many records in one SQLite file
//!
//! The rules that make it safe without any locks:
//!
//! - A file is written once and never changed. It appears under its name
//!   only when it is complete (written beside, then renamed).
//! - A running kernel writes only in its own directory.
//! - A record is named by its kernel and number, and two copies of a record
//!   are identical. So files may overlap, be copied between stores, or be
//!   read twice: a reader skips what it has. Putting two stores together is
//!   copying files (`merge`).
//!
//! `compact` turns everything in a store into one pack and deletes what the
//! pack replaces. When to do that, and anything else about looking after a
//! store, is for whoever looks after it to decide.

use crate::index::{self, Rec, Res};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub struct Store {
    pub dir: PathBuf,
    /// Files read already (also noted in the database, so it survives a restart).
    read: HashSet<String>,
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

/// The first and last record number a segment holds, from its name.
fn range_of(segment: &str) -> Option<(i64, i64)> {
    let (first, last) = segment.strip_suffix(".jsonl")?.split_once('-')?;
    Some((first.parse().ok()?, last.parse().ok()?))
}

fn open_pack(path: &Path) -> Res<Connection> {
    // `immutable`: nobody changes this file, so no locks are taken on it.
    // That is what makes reading it safe on network storage.
    let uri = format!("file:{}?immutable=1", path.display().to_string().replace('?', "%3f").replace('#', "%23"));
    Ok(Connection::open_with_flags(uri, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)?)
}

impl Store {
    pub fn open(dir: &Path, conn: &Connection) -> Res<Store> {
        fs::create_dir_all(dir).map_err(|e| format!("could not create the store at {}: {e}", dir.display()))?;
        let mut stmt = conn.prepare("SELECT path FROM sources")?;
        let read = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        Ok(Store { dir: dir.to_path_buf(), read })
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
    /// file. Returns how many there were.
    pub fn ship(&self, conn: &Connection, me: &str) -> Res<usize> {
        let shipped: i64 = index::get_meta(conn, "shipped")?.and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut stmt = conn.prepare_cached("SELECT n, at, kind, proc, data FROM records WHERE k = ?1 AND n > ?2 ORDER BY n")?;
        let mut text = String::new();
        let (mut first, mut last, mut count) = (0, 0, 0);
        let rows = stmt.query_map(params![me, shipped], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?))
        })?;
        for row in rows {
            let (n, at, kind, proc, data) = row?;
            if count == 0 {
                first = n;
            }
            last = n;
            count += 1;
            let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            text.push_str(&Rec { k: me.to_string(), n: Some(n), at, kind, proc, data }.to_value().to_string());
            text.push('\n');
        }
        if count == 0 {
            return Ok(0);
        }
        write_whole(&self.dir.join(me).join("log").join(format!("{first:012}-{last:012}.jsonl")), text.as_bytes())?;
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
        // Packs first: they hold the older records.
        for pack in names(&self.dir.join("packs")) {
            let key = format!("packs/{pack}");
            if !pack.ends_with(".db") || self.read.contains(&key) {
                continue;
            }
            // A pack that vanished meanwhile was replaced by a newer one.
            let Ok(pack) = open_pack(&self.dir.join("packs").join(&pack)) else { continue };
            let read = (|| -> Res<usize> {
                let mut count = 0;
                let mut kernels = pack.prepare("SELECT name, identity, created_at FROM kernels")?;
                for row in kernels.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?)))? {
                    let (name, identity, created_at) = row?;
                    know_kernel(conn, &name, identity.as_deref(), created_at)?;
                }
                let mut stmt = pack.prepare("SELECT k, n, at, kind, proc, data FROM records ORDER BY at, k, n")?;
                let rows = stmt.query_map([], |r| {
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
                    let rec = rec?;
                    if Some(rec.k.as_str()) != me && index::insert(conn, &rec)?.is_some() {
                        count += 1;
                    }
                }
                Ok(count)
            })();
            new += self.in_one_go(conn, &key, read)?;
        }
        for kernel in names(&self.dir) {
            if kernel == "packs" || Some(kernel.as_str()) == me {
                continue;
            }
            let dir = self.dir.join(&kernel);
            let about = format!("{kernel}/kernel.json");
            if !self.read.contains(&about) {
                if let Ok(text) = fs::read_to_string(dir.join("kernel.json")) {
                    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                    know_kernel(conn, &kernel, v["identity"].as_str(), v["created_at"].as_i64())?;
                    self.note_read(conn, &about)?;
                }
            }
            for segment in names(&dir.join("log")) {
                let key = format!("{kernel}/log/{segment}");
                if self.read.contains(&key) || range_of(&segment).is_none() {
                    continue;
                }
                let Ok(file) = fs::File::open(dir.join("log").join(&segment)) else { continue };
                let read = (|| -> Res<usize> {
                    let mut count = 0;
                    for line in BufReader::new(file).lines() {
                        let Ok(value) = serde_json::from_str::<Value>(&line?) else { continue };
                        let Some(rec) = Rec::from_value(&value) else { continue };
                        if rec.n.is_some() && index::insert(conn, &rec)?.is_some() {
                            count += 1;
                        }
                    }
                    Ok(count)
                })();
                new += self.in_one_go(conn, &key, read)?;
            }
        }
        Ok(new)
    }

    /// A file that was read whole is not read again.
    fn in_one_go(&mut self, conn: &Connection, key: &str, read: Res<usize>) -> Res<usize> {
        match read {
            Ok(count) => {
                self.note_read(conn, key)?;
                Ok(count)
            }
            // A file that could not be read is tried again next time.
            Err(e) => {
                eprintln!("jarvis: could not read {key} from the store: {e}");
                Ok(0)
            }
        }
    }

    fn note_read(&mut self, conn: &Connection, key: &str) -> Res<()> {
        conn.execute("INSERT OR IGNORE INTO sources (path) VALUES (?1)", [key])?;
        self.read.insert(key.to_string());
        Ok(())
    }
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

/// Put everything in the store into one pack, and delete what it replaces.
/// Records named in `without` are left out, and so are gone from this store.
pub fn compact(dir: &Path, without: &[String]) -> Res<String> {
    let packs_dir = dir.join("packs");
    fs::create_dir_all(&packs_dir)?;
    // What is there now. Files that appear while this runs are left alone.
    let old_packs: Vec<PathBuf> = names(&packs_dir).into_iter().filter(|n| n.ends_with(".db")).map(|n| packs_dir.join(n)).collect();
    let mut segments: Vec<PathBuf> = Vec::new();
    let mut kernels: Vec<(String, Value)> = Vec::new();
    for kernel in names(dir) {
        if kernel == "packs" {
            continue;
        }
        let about = fs::read_to_string(dir.join(&kernel).join("kernel.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        kernels.push((kernel.clone(), about));
        let log = dir.join(&kernel).join("log");
        segments.extend(names(&log).into_iter().filter(|n| range_of(n).is_some()).map(|n| log.join(n)));
    }
    if segments.is_empty() && old_packs.len() <= 1 && without.is_empty() {
        return Ok("nothing to compact".into());
    }
    let without: HashSet<(String, i64)> = without
        .iter()
        .map(|id| match id.rsplit_once(':') {
            Some((k, n)) if n.parse::<i64>().is_ok() => Ok((k.to_string(), n.parse().unwrap())),
            _ => Err(format!("`{id}` does not name a record: write KERNEL:NUMBER, such as laptop:412")),
        })
        .collect::<Result<_, _>>()?;

    let beside = packs_dir.join(format!(".tmp-{}-{}.db", std::process::id(), index::random_hex(4)?));
    let (mut kept, mut dropped) = (0usize, 0usize);
    let mut build = || -> Res<()> {
        let pack = Connection::open(&beside)?;
        pack.execute_batch(
            "CREATE TABLE kernels (name TEXT PRIMARY KEY, identity TEXT, created_at INTEGER);
             CREATE TABLE records (k TEXT NOT NULL, n INTEGER NOT NULL, at INTEGER NOT NULL, kind TEXT NOT NULL,
                                   proc TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (k, n)) WITHOUT ROWID;
             BEGIN;",
        )?;
        let mut add = |k: &str, n: i64, at: i64, kind: &str, proc: &str, data: &str| -> Res<()> {
            if without.contains(&(k.to_string(), n)) {
                dropped += 1;
                return Ok(());
            }
            kept += pack.prepare_cached("INSERT OR IGNORE INTO records (k, n, at, kind, proc, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?.execute(params![k, n, at, kind, proc, data])?;
            Ok(())
        };
        for path in &old_packs {
            let old = open_pack(path)?;
            let mut names = old.prepare("SELECT name, identity, created_at FROM kernels")?;
            for row in names.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?)))? {
                let (name, identity, created_at) = row?;
                pack.execute("INSERT OR IGNORE INTO kernels (name, identity, created_at) VALUES (?1, ?2, ?3)", params![name, identity, created_at])?;
            }
            let mut stmt = old.prepare("SELECT k, n, at, kind, proc, data FROM records")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                add(&r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, &r.get::<_, String>(3)?, &r.get::<_, String>(4)?, &r.get::<_, String>(5)?)?;
            }
        }
        for (name, about) in &kernels {
            pack.execute(
                "INSERT INTO kernels (name, identity, created_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(name) DO UPDATE SET identity = COALESCE(kernels.identity, excluded.identity)",
                params![name, about["identity"].as_str(), about["created_at"].as_i64()],
            )?;
        }
        for path in &segments {
            for line in BufReader::new(fs::File::open(path)?).lines() {
                let Ok(value) = serde_json::from_str::<Value>(&line?) else { continue };
                let Some(rec) = Rec::from_value(&value) else { continue };
                let Some(n) = rec.n else { continue };
                add(&rec.k, n, rec.at, &rec.kind, &rec.proc, &rec.data.to_string())?;
            }
        }
        pack.execute_batch("COMMIT;")?;
        Ok(())
    };
    if let Err(e) = build() {
        let _ = fs::remove_file(&beside);
        return Err(format!("could not write the pack, so nothing was changed: {e}").into());
    }
    let stamp = index::now_ms();
    let name = format!("{stamp:014}-{}.db", index::random_hex(3)?);
    fs::rename(&beside, packs_dir.join(&name))?;
    // Only now, with the pack in place, is what it replaces deleted. A crash
    // before this leaves records in two places, which harms nothing.
    for path in old_packs.iter().chain(&segments) {
        let _ = fs::remove_file(path);
    }
    let left_out = if dropped > 0 { format!(", {dropped} left out") } else { String::new() };
    Ok(format!("packs/{name}: {kept} records from {} file(s){left_out}", old_packs.len() + segments.len()))
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
    for pack in names(&from.join("packs")) {
        if pack.ends_with(".db") {
            copy(&from.join("packs").join(&pack), &dir.join("packs").join(&pack))?;
        }
    }
    for kernel in names(from) {
        if kernel == "packs" || !from.join(&kernel).is_dir() {
            continue;
        }
        let identity = |store: &Path| -> Option<Value> {
            let text = fs::read_to_string(store.join(&kernel).join("kernel.json")).ok()?;
            Some(serde_json::from_str::<Value>(&text).ok()?["identity"].clone())
        };
        // Two different kernels that were given the same name cannot share a directory.
        if let (Some(theirs), Some(ours)) = (identity(from), identity(dir)) {
            if theirs != ours {
                refused.push(kernel);
                continue;
            }
        }
        if from.join(&kernel).join("kernel.json").exists() {
            copy(&from.join(&kernel).join("kernel.json"), &dir.join(&kernel).join("kernel.json"))?;
        }
        for segment in names(&from.join(&kernel).join("log")) {
            if range_of(&segment).is_some() {
                copy(&from.join(&kernel).join("log").join(&segment), &dir.join(&kernel).join("log").join(&segment))?;
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

        // A reader that saw the files before they were packed, and one after.
        let early = index::open_memory().unwrap();
        let mut early_reader = Store::open(&one, &early).unwrap();
        early_reader.ingest(&early, None).unwrap();

        compact(&one, &["a:7".to_string()]).unwrap();
        assert!(names(&one.join("a").join("log")).is_empty());
        merge(&one, &two).unwrap();
        compact(&one, &[]).unwrap();
        assert_eq!(names(&one.join("packs")).len(), 1);

        let late = index::open_memory().unwrap();
        Store::open(&one, &late).unwrap().ingest(&late, None).unwrap();
        assert_eq!(messages(&late), 69);
        early_reader.ingest(&early, None).unwrap();
        assert_eq!(messages(&early), 70);
        for dir in [one, two] {
            fs::remove_dir_all(dir).unwrap();
        }
    }
}
