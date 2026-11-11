//! Carrying out `jarvis` commands.
//!
//! A command typed anywhere is sent, as it was typed, to a kernel's socket.
//! A thread here parses it (with the same definitions as the command line,
//! `cli.rs`) and does what it says. Reading is done right here, on the
//! thread's own connection to the kernel's database. Anything that changes
//! something is handed to the kernel's loop as a `Mutation`, because the loop
//! is the only writer. A command for another kernel is handed on over the
//! links, and its answer printed as if it had been carried out here.
//!
//! The same code answers a reader that has no kernel (`--store DIR`): it is
//! given a database built from the store and nothing to change things with.

use crate::children;
use crate::cli::{self, Cli, Cmd, StoreCmd};
use crate::git::Repo;
use crate::index::{self, HUMAN, KERNEL, Proc, Res, at, kernel_of, name_of};
use crate::kernel::{self, Config, Event, Mutation, Shared};
use crate::net;
use crate::store;
use clap::Parser;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

/// Where a command's output goes: back over the socket as it is produced, or
/// into a list (for a command carried out on behalf of another kernel).
pub trait Out {
    /// A line of output. False when nobody is reading any more.
    fn line(&mut self, text: &str) -> bool;
    /// Something for standard error.
    fn note(&mut self, text: &str);
}

struct ToSocket(UnixStream);

impl Out for ToSocket {
    fn line(&mut self, text: &str) -> bool {
        writeln!(self.0, "{}", json!({"o": text})).is_ok()
    }
    fn note(&mut self, text: &str) {
        let _ = writeln!(self.0, "{}", json!({"e": text}));
    }
}

#[derive(Default)]
pub struct Collected {
    pub lines: Vec<String>,
    pub notes: Vec<String>,
}

impl Out for Collected {
    fn line(&mut self, text: &str) -> bool {
        self.lines.push(text.to_string());
        true
    }
    fn note(&mut self, text: &str) {
        self.notes.push(text.to_string());
    }
}

/// What a command is carried out with.
pub struct Ctx {
    /// This kernel's name; empty when reading a store with no kernel.
    pub me: String,
    pub conn: Connection,
    pub cfg: Option<Arc<Config>>,
    pub repo: Option<Repo>,
    tx: Option<Sender<Event>>,
    shared: Option<Arc<Shared>>,
}

impl Ctx {
    fn of_kernel(cfg: Arc<Config>, tx: Sender<Event>, shared: Arc<Shared>) -> Res<Ctx> {
        Ok(Ctx { me: cfg.name.clone(), conn: index::open(&cfg.db_path())?, repo: Some(cfg.repo.clone()), cfg: Some(cfg), tx: Some(tx), shared: Some(shared) })
    }

    /// For reading a store directly: a database built from it, and no kernel.
    pub fn of_store(conn: Connection, repo: Option<Repo>) -> Ctx {
        Ctx { me: String::new(), conn, cfg: None, repo, tx: None, shared: None }
    }

    /// Have the kernel change something, and wait for its answer.
    fn mutate(&self, mutation: Mutation) -> Res<Value> {
        let tx = self.tx.as_ref().ok_or("this changes something, and so needs a running kernel (it cannot be done on a store alone)")?;
        let (reply, answer) = mpsc::channel();
        tx.send(Event::Mutate(mutation, reply)).map_err(|_| "the kernel is stopping")?;
        Ok(answer.recv().map_err(|_| "the kernel stopped before it answered")??)
    }

    /// Be told whenever there are new records.
    fn follow(&self) -> Res<Receiver<()>> {
        let shared = self.shared.as_ref().ok_or("following needs a running kernel")?;
        let (tx, rx) = mpsc::sync_channel(1);
        shared.followers.lock().unwrap().push(tx);
        Ok(rx)
    }

    fn repo(&self) -> Res<&Repo> {
        self.repo.as_ref().ok_or_else(|| "this needs the git repository; run it inside one".into())
    }

    /// How to call an address in what people read: without the kernel when it is this one.
    fn label(&self, addr: &str) -> String {
        if !self.me.is_empty() && kernel_of(addr) == self.me { name_of(addr).to_string() } else { addr.to_string() }
    }
}

/// Take the kernel's socket and answer what arrives on it, from now on.
/// Fails if a kernel of this name is already running on this machine.
pub fn listen(cfg: Arc<Config>, tx: Sender<Event>, shared: Arc<Shared>) -> Res<()> {
    let path = cfg.run_dir.join(format!("{}.sock", cfg.name));
    if UnixStream::connect(&path).is_ok() {
        return Err(format!("a kernel called `{}` is already running on this machine", cfg.name).into());
    }
    let _ = fs::remove_file(&path);
    let listener = UnixListener::bind(&path).map_err(|e| format!("could not make the socket {}: {e}. (A socket's path must be short; set $JARVIS_RUN to a short directory on this machine's own disk.)", path.display()))?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (cfg, tx, shared) = (cfg.clone(), tx.clone(), shared.clone());
            thread::spawn(move || connection(stream, cfg, tx, shared));
        }
    });
    Ok(())
}

fn connection(stream: UnixStream, cfg: Arc<Config>, tx: Sender<Event>, shared: Arc<Shared>) {
    let Ok(read_half) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_half);
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let Ok(request) = serde_json::from_str::<Value>(&first) else { return };
    // Not a command: another kernel, asking to be linked.
    if request["link"] == true {
        let id = shared.next_link.fetch_add(1, Ordering::SeqCst);
        return net::accept(stream, reader, id, &tx);
    }
    let mut out = ToSocket(stream);
    let code = match Ctx::of_kernel(cfg, tx, shared) {
        Ok(ctx) => handle(&ctx, &request, &mut out),
        Err(e) => {
            out.note(&format!("jarvis: {e}"));
            1
        }
    };
    let _ = writeln!(out.0, "{}", json!({"end": code}));
}

/// Carry out a command another kernel sent over the links, and return what
/// it printed.
pub fn answer(cfg: Arc<Config>, tx: Sender<Event>, shared: Arc<Shared>, request: Value) -> Value {
    let mut out = Collected::default();
    let code = match Ctx::of_kernel(cfg, tx, shared) {
        Ok(ctx) => handle(&ctx, &request, &mut out),
        Err(e) => {
            out.note(&format!("jarvis: {e}"));
            1
        }
    };
    json!({"o": out.lines, "e": out.notes, "code": code})
}

/// Carry out one command. The request is `{"argv": what was typed after
/// `jarvis`, "by": who typed it, "cwd": where, "stdin": what was piped in,
/// "files": the content of files it names}`. Returns the exit code.
pub fn handle(ctx: &Ctx, request: &Value, out: &mut dyn Out) -> i32 {
    match run(ctx, request, out) {
        Ok(code) => code,
        Err(e) => {
            out.note(&format!("jarvis: {e}"));
            1
        }
    }
}

fn print(out: &mut dyn Out, answer: &Value) {
    for line in answer["o"].as_array().into_iter().flatten() {
        out.line(line.as_str().unwrap_or(""));
    }
    for note in answer["e"].as_array().into_iter().flatten() {
        out.note(&format!("note: {}", note.as_str().unwrap_or("")));
    }
}

fn run(ctx: &Ctx, request: &Value, out: &mut dyn Out) -> Res<i32> {
    let argv: Vec<String> = request["argv"].as_array().map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let cli = Cli::try_parse_from(std::iter::once("jarvis".to_string()).chain(argv)).map_err(|e| e.to_string())?;
    let command = cli.command.ok_or("no command was given")?;
    // Who is asking: a process (its address), or the person at this kernel.
    let by = match request["by"].as_str().unwrap_or("") {
        "" | HUMAN => at(HUMAN, &ctx.me),
        address if address.contains('@') => address.to_string(),
        name => at(name, &ctx.me),
    };

    // A command for another kernel is handed on, and its answer printed here.
    if let Some(on) = target(ctx, &cli.on, &command)? {
        if on != ctx.me {
            if matches!(&command, Cmd::Watch(_)) || matches!(&command, Cmd::Log(l) if l.follow) {
                return Err(format!("another kernel cannot be followed from here. Run this where `{on}` runs, or follow this kernel: it shows `{on}`'s records as they reach the store").into());
            }
            let mut passed = request.clone();
            passed["by"] = json!(by);
            let answer = ctx.mutate(Mutation::Remote { on, request: passed })?;
            for line in answer["o"].as_array().into_iter().flatten() {
                out.line(line.as_str().unwrap_or(""));
            }
            for note in answer["e"].as_array().into_iter().flatten() {
                out.note(note.as_str().unwrap_or(""));
            }
            return Ok(answer["code"].as_i64().unwrap_or(1) as i32);
        }
    }

    match command {
        Cmd::Up(_) => return Err("`jarvis up` starts a kernel; it is not something a kernel is asked to do".into()),
        Cmd::Spawn(args) => {
            let prompt = match (&args.prompt, &args.prompt_file) {
                (Some(text), _) => Some(text.clone()),
                (None, Some(file)) => Some(request["files"][file].as_str().ok_or_else(|| format!("could not read {file}"))?.to_string()),
                (None, None) => None,
            };
            let cwd = request["cwd"].as_str().unwrap_or("/").to_string();
            print(out, &ctx.mutate(Mutation::Spawn { args, by, cwd, prompt })?);
        }
        Cmd::Ps(args) => ps(ctx, args, out)?,
        Cmd::Stop(args) => print(out, &ctx.mutate(Mutation::SetStatus { who: args.who, alive: false, by })?),
        Cmd::Start(args) => print(out, &ctx.mutate(Mutation::SetStatus { who: args.who, alive: true, by })?),
        Cmd::Remove(args) => {
            for who in args.who {
                print(out, &ctx.mutate(Mutation::Remove { who, keep_branch: args.keep_branch, by: by.clone() })?);
            }
        }
        Cmd::Tag(args) => {
            let tags: Vec<(String, String)> = args.tags.iter().map(|t| parse_tag(t)).collect::<Res<_>>()?;
            if !tags.is_empty() {
                ctx.mutate(Mutation::Tag { who: args.who.clone(), tags, by })?;
            }
            let proc = one(ctx, &args.who)?;
            match tags_of(&ctx.conn, &proc)?.as_object() {
                Some(now) => {
                    for (key, value) in now {
                        out.line(&format!("{key}={}", value.as_str().unwrap_or("")));
                    }
                }
                None => {}
            }
        }
        Cmd::Send(args) => {
            let mut body = args.text.join(" ");
            if body == "-" || body.is_empty() {
                body = request["stdin"].as_str().unwrap_or("").to_string();
            }
            if body.trim().is_empty() {
                return Err("there was nothing to send: the message was empty".into());
            }
            let from = match args.sender.as_deref() {
                None => by,
                Some(who) if who.contains('@') => who.to_string(),
                Some(who) => at(who, &ctx.me),
            };
            print(out, &ctx.mutate(Mutation::Send { from, to: args.to, body: body.trim_end().to_string() })?);
        }
        Cmd::Messages(args) => messages(ctx, args, out)?,
        Cmd::Show(args) => show(ctx, args, out)?,
        Cmd::Log(args) => log(ctx, args, out)?,
        Cmd::Watch(args) => watch(ctx, args, out)?,
        Cmd::Kernels(args) => {
            if let Some(name) = args.close {
                print(out, &ctx.mutate(Mutation::CloseKernel { name, by })?);
            } else {
                kernels(ctx, args.json, out)?;
            }
        }
        Cmd::Link(args) => {
            if args.stdio {
                return Err("`jarvis link --stdio` is the far end of a link; it is not something a kernel is asked to do".into());
            }
            let spec = match (&args.kernel, args.command.is_empty()) {
                (Some(kernel), true) => Some(json!({"kernel": kernel}).to_string()),
                (None, false) => Some(json!({"command": args.command}).to_string()),
                (Some(_), false) => return Err("name a kernel on this machine, or give a command after `--`, not both".into()),
                (None, true) => None,
            };
            match spec {
                Some(spec) => print(out, &ctx.mutate(Mutation::Link { spec, remove: args.remove })?),
                None if args.remove => return Err("say which link to forget, as it was made".into()),
                None => links(ctx, args.json, out)?,
            }
        }
        Cmd::Store(args) => {
            let dir = || -> Res<&Path> { Ok(ctx.cfg.as_ref().ok_or("no kernel, and so no store, here")?.store_dir.as_path()) };
            match args.what {
                StoreCmd::Ship => print(out, &ctx.mutate(Mutation::Ship)?),
                StoreCmd::Compact { without } => {
                    out.line(&store::compact(dir()?, &without)?);
                }
                StoreCmd::Merge { other } => {
                    let other = Path::new(request["cwd"].as_str().unwrap_or("/")).join(other);
                    out.line(&store::merge(dir()?, &other)?);
                }
            }
        }
    }
    Ok(0)
}

/// The kernel a command is for, when it is said or follows from who it names.
fn target(ctx: &Ctx, on: &Option<String>, command: &Cmd) -> Res<Option<String>> {
    if let Some(on) = on {
        return Ok(Some(on.clone()));
    }
    // Changing a process is done by the kernel that hosts it.
    let who: Vec<&String> = match command {
        Cmd::Stop(args) | Cmd::Start(args) => args.who.iter().collect(),
        Cmd::Remove(args) => args.who.iter().collect(),
        Cmd::Tag(args) if !args.tags.is_empty() => vec![&args.who],
        _ => return Ok(None),
    };
    let kernels: BTreeSet<&str> = who.iter().map(|w| if w.contains('@') { kernel_of(w) } else { ctx.me.as_str() }).collect();
    match kernels.len() {
        1 => Ok(kernels.into_iter().next().map(str::to_string)),
        _ => Err("these processes are on different kernels; give the command once for each kernel".into()),
    }
}

// ---------------------------------------------------------------- helpers

/// The one process `who` stands for: an address, or a bare name, which means
/// this kernel's process of that name, or else the only one there is.
fn one(ctx: &Ctx, who: &str) -> Res<Proc> {
    if who.contains('@') {
        return index::find_proc(&ctx.conn, who)?.ok_or_else(|| format!("there is no process `{who}` (`jarvis ps` lists them)").into());
    }
    if !ctx.me.is_empty() {
        if let Some(proc) = index::find_proc(&ctx.conn, &at(who, &ctx.me))? {
            return Ok(proc);
        }
    }
    let mut all = index::procs_named(&ctx.conn, who)?;
    match all.len() {
        0 => Err(format!("there is no process called `{who}` (`jarvis ps` lists them)").into()),
        1 => Ok(all.pop().expect("there is one")),
        _ => Err(format!(
            "several processes are called `{who}`: {}. Say which one",
            all.iter().map(|p| p.addr.as_str()).collect::<Vec<_>>().join(", ")
        )
        .into()),
    }
}

/// The address to look records up by: `who` as given if it is one (also of a
/// process that is gone), else the one process of that name.
fn address(ctx: &Ctx, who: &str) -> Res<String> {
    if who.contains('@') {
        return Ok(who.to_string());
    }
    if matches!(who, HUMAN | KERNEL) && !ctx.me.is_empty() {
        return Ok(at(who, &ctx.me));
    }
    Ok(one(ctx, who)?.addr)
}

/// The first line of real text, shortened.
fn clip(text: &str, limit: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > limit { format!("{}…", line.chars().take(limit).collect::<String>()) } else { line.to_string() }
}

/// `2m`, `40s`: how long ago a moment (in milliseconds) was.
fn ago(ms: i64) -> String {
    let seconds = (index::now_ms() - ms).max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..172800 => format!("{}h{}m", seconds / 3600, seconds % 3600 / 60),
        _ => format!("{}d", seconds / 86400),
    }
}

/// The first line of real text in a process's `.jarvis/ABOUT.md`: how it
/// describes itself. From its directory if it is here, else from its branch.
fn about(ctx: &Ctx, p: &Proc) -> String {
    let text = match &ctx.cfg {
        Some(cfg) if p.kernel == ctx.me => fs::read_to_string(cfg.proc_dir(&p.name).join(".jarvis/ABOUT.md")).unwrap_or_default(),
        _ => ctx.repo.as_ref().and_then(|repo| repo.git(&["show", &format!("{}:.jarvis/ABOUT.md", p.branch)]).ok()).unwrap_or_default(),
    };
    text.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).unwrap_or("").to_string()
}

/// How far a message has got, in words; empty if it is dealt with.
fn progress(status: &str, attempts: i64) -> String {
    match (status, attempts) {
        ("handled", _) => String::new(),
        ("sent", _) => "on its way".into(),
        ("forwarded", _) => "passed on".into(),
        ("taken", 0 | 1) => "being worked on".into(),
        ("taken", n) => format!("being worked on, attempt {n}"),
        ("dropped", _) => "dropped: its recipient was removed".into(),
        ("undeliverable", _) => "could not be delivered".into(),
        (_, 0) => "waiting".into(),
        (_, 1) => "waiting; one turn on it did not finish".into(),
        (_, n) => format!("waiting; {n} turns on it did not finish"),
    }
}

/// `NAME=VALUE` as a tag. An empty value means "remove this tag".
pub fn parse_tag(text: &str) -> Res<(String, String)> {
    let (key, value) = text.split_once('=').ok_or_else(|| format!("`{text}` is not a tag: write NAME=VALUE, such as project=moe"))?;
    let key = key.trim();
    let plain = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.');
    if key.is_empty() || !key.chars().all(plain) {
        return Err(format!("`{key}` cannot be the name of a tag: use letters, digits, spaces, `-`, `_` and `.`").into());
    }
    if key == "kernel" {
        return Err("`kernel` is not a tag to set: every process counts as tagged with the kernel it is on".into());
    }
    Ok((key.to_string(), value.trim().to_string()))
}

/// The tags of a process, as a JSON object. The kernel it is on is one of them.
fn tags_of(conn: &Connection, p: &Proc) -> Res<Value> {
    let mut stmt = conn.prepare_cached("SELECT key, value FROM tags WHERE addr = ?1 ORDER BY key")?;
    let mut tags: serde_json::Map<String, Value> =
        stmt.query_map([&p.addr], |r| Ok((r.get::<_, String>(0)?, Value::String(r.get(1)?))))?.collect::<Result<_, _>>()?;
    tags.insert("kernel".into(), Value::String(p.kernel.clone()));
    Ok(Value::Object(tags))
}

// --------------------------------------------------------------------- ps

/// Everything `ps` knows about one process.
fn describe(ctx: &Ctx, p: &Proc) -> Res<Value> {
    let conn = &ctx.conn;
    let here = p.kernel == ctx.me;
    let dir = ctx.cfg.as_ref().filter(|_| here).map(|cfg| cfg.proc_dir(&p.name));
    // What its agent has cost so far, as far as its harness reported.
    let (turns, cost): (i64, Option<f64>) = conn
        .prepare_cached("SELECT COUNT(*), SUM(json_extract(data, '$.cost_usd')) FROM records WHERE proc = ?1 AND kind = 'turn'")?
        .query_row([&p.addr], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mail: i64 = conn
        .prepare_cached("SELECT COUNT(*) FROM messages WHERE recipient = ?1 AND status IN ('pending', 'taken')")?
        .query_row([&p.addr], |r| r.get(0))?;
    // `failing`: its last turn failed, and its mail is waiting for the next try.
    let state = if p.status != "alive" || index::kernel_closed(conn, &p.kernel) {
        "stopped"
    } else if p.busy {
        "thinking"
    } else if p.failures > 0 {
        "failing"
    } else {
        "idle"
    };
    Ok(json!({
        "id": p.addr, "name": p.name, "kernel": p.kernel, "here": here, "state": state, "about": about(ctx, p),
        "from": p.from, "tags": tags_of(conn, p)?, "branch": p.branch,
        "directory": dir.as_ref().map(|d| d.display().to_string()), "harness": p.harness,
        "parent": p.parent, "created_at": p.created_at,
        "turns": turns, "cost_usd": cost, "mail_waiting": mail,
        "busy_since": p.busy_since, "failures": p.failures,
        "has_run": dir.as_ref().map(|d| kernel::is_executable(&d.join(".jarvis/run"))),
        "has_handle": dir.as_ref().map(|d| kernel::is_executable(&d.join(".jarvis/handle"))),
    }))
}

/// A process's state as people read it: `thinking 3m`, `failing x4`.
fn state_words(row: &Value) -> String {
    let state = row["state"].as_str().unwrap_or("");
    match (state, row["busy_since"].as_i64()) {
        ("thinking", Some(since)) => format!("thinking {}", ago(since)),
        ("failing", _) => format!("failing x{}", row["failures"]),
        _ => state.to_string(),
    }
}

fn ps(ctx: &Ctx, args: cli::Ps, out: &mut dyn Out) -> Res<()> {
    if let Some(who) = &args.who {
        return ps_one(ctx, &one(ctx, who)?, args.json, out);
    }
    // A store read on its own is history: the kernels in it need not be running.
    let mut procs = index::procs(&ctx.conn, args.all || ctx.me.is_empty())?;
    // This kernel's first.
    procs.sort_by_key(|p| p.kernel != ctx.me);
    let rows: Vec<Value> = procs.iter().map(|p| describe(ctx, p)).collect::<Res<_>>()?;
    if args.json {
        out.line(&Value::Array(rows).to_string());
        return Ok(());
    }
    if rows.is_empty() {
        out.line("no processes yet. `jarvis help spawn` shows how to create one.");
        return Ok(());
    }
    out.line(&format!("{:<24} {:<13} {:<16} ABOUT", "NAME", "STATE", "MADE BY"));
    for r in rows {
        out.line(&format!(
            "{:<24} {:<13} {:<16} {}",
            ctx.label(r["id"].as_str().unwrap_or("-")),
            state_words(&r),
            ctx.label(r["parent"].as_str().unwrap_or("")),
            r["about"].as_str().unwrap_or(""),
        ));
    }
    Ok(())
}

/// Everything about one process.
fn ps_one(ctx: &Ctx, p: &Proc, as_json: bool, out: &mut dyn Out) -> Res<()> {
    let conn = &ctx.conn;
    let mut row = describe(ctx, p)?;
    let here = p.kernel == ctx.me;
    let service_pid: Option<u32> = if here {
        conn.query_row("SELECT pid, started FROM children WHERE proc = ?1 AND kind = 'run'", [&p.name], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)))
            .ok()
            .filter(|(pid, since)| children::is_running(*pid, since))
            .map(|(pid, _)| pid)
    } else {
        None
    };
    let last_of = |kinds: &str| -> Option<(i64, String, Value)> {
        conn.query_row(
            &format!("SELECT at, kind, data FROM records WHERE proc = ?1 AND kind IN ({kinds}) ORDER BY seq DESC LIMIT 1"),
            [&p.addr],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?)),
        )
        .ok()
        .map(|(at, kind, data)| (at, kind, serde_json::from_str(&data).unwrap_or(Value::Null)))
    };
    let last_turn = last_of("'turn', 'turn_failed', 'turn_interrupted'");
    let last_exit = last_of("'run_exit'");
    let head = ctx.repo.as_ref().and_then(|repo| repo.git(&["rev-parse", "--short", &p.branch]).ok());
    row["commit"] = json!(head);
    row["service_pid"] = json!(service_pid);
    row["last_turn"] = match &last_turn {
        Some((at, kind, data)) => json!({"at": at, "kind": kind, "reply": data["reply"], "error": data["error"], "cost_usd": data["cost_usd"]}),
        None => Value::Null,
    };
    if as_json {
        out.line(&row.to_string());
        return Ok(());
    }

    let mut line = |key: &str, value: String| {
        out.line(&format!("{key:<10} {value}"));
    };
    line("address", p.addr.clone());
    line("state", state_words(&row));
    line("about", match row["about"].as_str() {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => "(has not described itself yet)".into(),
    });
    line("made by", format!("{}, {} ago", ctx.label(&p.parent), ago(p.created_at)));
    if let Some(from) = &p.from {
        line("from", from.clone());
    }
    line("harness", p.harness.clone());
    if let Some(tags) = row["tags"].as_object() {
        line("tags", tags.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
    }
    line("branch", format!("{}{}", p.branch, head.map(|h| format!(" (at {h})")).unwrap_or_default()));
    if here {
        line("directory", row["directory"].as_str().unwrap_or("").to_string());
        line("service", match (row["has_run"].as_bool(), service_pid, &last_exit) {
            (Some(true), Some(pid), _) => format!("running (pid {pid})"),
            (Some(true), None, Some((at, _, data))) => format!("not running; it exited {} ago with {}", ago(*at), data["status"].as_str().unwrap_or("?")),
            (Some(true), None, None) => "not running".into(),
            _ => "none (no .jarvis/run)".into(),
        });
        line("handler", if row["has_handle"] == true { "yes (.jarvis/handle)".into() } else { "none".into() });
    } else {
        line("kernel", format!("{}: not this one, so this is what its records say", p.kernel));
    }
    line("turns", match row["cost_usd"].as_f64() {
        Some(cost) => format!("{}, costing ${cost:.2} in all", row["turns"]),
        None => format!("{}", row["turns"]),
    });
    line("mail", format!("{} waiting", row["mail_waiting"]));
    match last_turn {
        None => line("last turn", "none yet".into()),
        Some((at, kind, data)) => match kind.as_str() {
            "turn" => line("last turn", format!("{} ago: {}", ago(at), clip(data["reply"].as_str().unwrap_or(""), 200))),
            "turn_interrupted" => line("last turn", format!("{} ago: interrupted (the process was stopped)", ago(at))),
            _ => {
                line("last turn", format!("{} ago: FAILED, {} in a row. The error:", ago(at), data["failures"]));
                for text in data["error"].as_str().unwrap_or("").lines() {
                    line("", text.to_string());
                }
            }
        },
    }
    Ok(())
}

// ---------------------------------------------------------------- kernels

fn kernels(ctx: &Ctx, as_json: bool, out: &mut dyn Out) -> Res<()> {
    let net = ctx.shared.as_ref().map(|s| s.net.lock().unwrap().clone()).unwrap_or_default();
    let mut stmt = ctx.conn.prepare(
        "SELECT name, closed, closed_by, last_at,
                (SELECT COUNT(*) FROM procs WHERE kernel = kernels.name AND status != 'removed')
         FROM kernels ORDER BY name",
    )?;
    let known: Vec<(String, bool, Option<String>, i64, i64)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<Result<_, _>>()?;
    let mut rows = Vec::new();
    for (name, closed, closed_by, last_at, procs) in known {
        let reach = net.reachable.iter().find(|r| r.name == name);
        let state = if name == ctx.me {
            "here".to_string()
        } else if let Some(reach) = reach {
            match reach.hops {
                1 => "linked".to_string(),
                hops => format!("reachable through {} ({hops} links away)", reach.via),
            }
        } else if closed {
            match closed_by.as_deref() {
                Some(by) if name_of(by) == KERNEL && kernel_of(by) == name => "closed".to_string(),
                Some(by) => format!("taken to be gone (said {by})"),
                None => "closed".to_string(),
            }
        } else {
            "not reachable".to_string()
        };
        rows.push(json!({"name": name, "state": state, "here": name == ctx.me, "reachable": name == ctx.me || reach.is_some(),
                         "hops": reach.map(|r| r.hops), "via": reach.map(|r| r.via.clone()), "closed": closed,
                         "last_wrote_at": last_at, "processes": procs}));
    }
    // A kernel that is linked but whose records have not been seen yet.
    for reach in &net.reachable {
        if !rows.iter().any(|r| r["name"] == reach.name.as_str()) {
            rows.push(json!({"name": reach.name, "state": if reach.hops == 1 { "linked".to_string() } else { format!("reachable through {} ({} links away)", reach.via, reach.hops) },
                             "here": false, "reachable": true, "hops": reach.hops, "via": reach.via, "closed": false,
                             "last_wrote_at": Value::Null, "processes": reach.procs.len()}));
        }
    }
    if as_json {
        out.line(&Value::Array(rows).to_string());
        return Ok(());
    }
    out.line(&format!("{:<20} {:<10} {:<12} STATE", "KERNEL", "PROCESSES", "LAST WROTE"));
    for r in rows {
        let wrote = r["last_wrote_at"].as_i64().filter(|at| *at > 0).map(|at| format!("{} ago", ago(at))).unwrap_or_else(|| "-".into());
        out.line(&format!("{:<20} {:<10} {:<12} {}", r["name"].as_str().unwrap_or(""), r["processes"].to_string(), wrote, r["state"].as_str().unwrap_or("")));
    }
    Ok(())
}

fn links(ctx: &Ctx, as_json: bool, out: &mut dyn Out) -> Res<()> {
    let net = ctx.shared.as_ref().map(|s| s.net.lock().unwrap().clone()).unwrap_or_default();
    let rows: Vec<Value> = net
        .links
        .iter()
        .map(|l| {
            let spec: Value = l.spec.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null);
            let how = match (spec["kernel"].as_str(), spec["command"].as_array()) {
                (Some(kernel), _) => format!("to `{kernel}` on this machine"),
                (None, Some(words)) => format!("through `{}`", words.iter().filter_map(|w| w.as_str()).collect::<Vec<_>>().join(" ")),
                _ => "made from the other end".to_string(),
            };
            let state = match (&l.peer, &l.problem) {
                (Some(peer), _) => format!("up, to {peer}"),
                (None, Some(problem)) => format!("down: {problem}"),
                (None, None) => "being made".to_string(),
            };
            json!({"link": how, "kernel": l.peer, "up": l.peer.is_some(), "state": state})
        })
        .collect();
    if as_json {
        out.line(&Value::Array(rows).to_string());
    } else if rows.is_empty() {
        out.line("this kernel has no links. `jarvis help link` shows how to make one.");
    } else {
        for r in rows {
            out.line(&format!("{}: {}", r["link"].as_str().unwrap_or(""), r["state"].as_str().unwrap_or("")));
        }
    }
    Ok(())
}

// --------------------------------------------------------------- messages

/// Messages to or from `?N`: every process of that name if it is a bare
/// name, that one process if it is an address.
fn involves(n: usize) -> String {
    format!("(?{n} IS NULL OR CASE WHEN instr(?{n}, '@') > 0 THEN (sender = ?{n} OR recipient = ?{n}) ELSE (sn = ?{n} OR rn = ?{n}) END)")
}

const OPEN: &str = "status NOT IN ('handled', 'dropped', 'undeliverable')";

fn message_row(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>(0)?, "at": r.get::<_, i64>(1)?, "from": r.get::<_, String>(2)?,
        "to": r.get::<_, String>(3)?, "body": r.get::<_, String>(4)?, "status": r.get::<_, String>(5)?,
        "attempts": r.get::<_, i64>(6)?, "seq": r.get::<_, i64>(7)?,
    }))
}

/// `seq`: the number of the fact (as `jarvis log --json` and `jarvis watch`
/// number them) that first brought this message here.
const MESSAGE_COLUMNS: &str = "id, at, sender, recipient, body, status, attempts, seq";

fn message_json(conn: &Connection, id: &str) -> Option<Value> {
    conn.prepare_cached(&format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1")).ok()?.query_row([id], message_row).ok()
}

fn messages(ctx: &Ctx, args: cli::Messages, out: &mut dyn Out) -> Res<()> {
    let conn = &ctx.conn;
    if args.pairs {
        return pairs(ctx, args.json, out);
    }
    let count = args.count.unwrap_or(if args.json { 500 } else { 20 });
    let mut stmt = conn.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS}, strftime('%H:%M:%S', at / 1000, 'unixepoch', 'localtime')
         FROM messages
         WHERE (?1 IS NULL OR (at, id) > (SELECT at, id FROM messages WHERE id = ?1))
           AND (?2 IS NULL OR (at, id) < (SELECT at, id FROM messages WHERE id = ?2))
           AND (?3 = 0 OR {OPEN}) AND {} AND {}
         ORDER BY at DESC, id DESC LIMIT ?6",
        involves(4),
        involves(5)
    ))?;
    let mut rows: Vec<(Value, String)> = stmt
        .query_map(params![args.after, args.before, args.waiting, args.who, args.with, count], |r| Ok((message_row(r)?, r.get::<_, String>(8)?)))?
        .collect::<Result<_, _>>()?;
    rows.reverse();
    if args.json {
        out.line(&Value::Array(rows.into_iter().map(|(row, _)| row).collect()).to_string());
        return Ok(());
    }
    if rows.is_empty() {
        out.line(&format!("no messages{}", if args.waiting { " are waiting" } else { "" }));
    }
    for (m, time) in rows {
        let state = progress(m["status"].as_str().unwrap_or(""), m["attempts"].as_i64().unwrap_or(0));
        out.line(&format!(
            "#{} {time} {} → {}{}\n{}\n",
            m["id"].as_str().unwrap_or(""),
            ctx.label(m["from"].as_str().unwrap_or("")),
            ctx.label(m["to"].as_str().unwrap_or("")),
            if state.is_empty() { String::new() } else { format!("  ({state})") },
            m["body"].as_str().unwrap_or(""),
        ));
    }
    Ok(())
}

/// Who has written to whom, and how much.
fn pairs(ctx: &Ctx, as_json: bool, out: &mut dyn Out) -> Res<()> {
    let mut stmt = ctx.conn.prepare(&format!(
        "SELECT MIN(sender, recipient), MAX(sender, recipient), COUNT(*), SUM({OPEN}),
                SUM(body LIKE '[jarvis]%'), MAX(at), MAX(seq)
         FROM messages WHERE sender != recipient GROUP BY 1, 2 ORDER BY 6"
    ))?;
    let rows: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "a": r.get::<_, String>(0)?, "b": r.get::<_, String>(1)?, "count": r.get::<_, i64>(2)?,
                "waiting": r.get::<_, i64>(3)?, "by_jarvis": r.get::<_, i64>(4)?, "last_at": r.get::<_, i64>(5)?,
                "last_seq": r.get::<_, i64>(6)?,
            }))
        })?
        .collect::<Result<_, _>>()?;
    if as_json {
        out.line(&Value::Array(rows).to_string());
        return Ok(());
    }
    if rows.is_empty() {
        out.line("nobody has written to anybody yet");
    }
    for r in rows {
        let waiting = match r["waiting"].as_i64().unwrap_or(0) {
            0 => String::new(),
            n => format!(", {n} not dealt with"),
        };
        out.line(&format!(
            "{} and {}: {} message(s){waiting}",
            ctx.label(r["a"].as_str().unwrap_or("")),
            ctx.label(r["b"].as_str().unwrap_or("")),
            r["count"]
        ));
    }
    Ok(())
}

// ------------------------------------------------------------------- show

fn show(ctx: &Ctx, args: cli::Show, out: &mut dyn Out) -> Res<()> {
    let repo = ctx.repo()?;
    // A branch outlives its process and its kernel: an address whose branch
    // is still there can be looked at whatever became of the process.
    let branch = match one(ctx, &args.who) {
        Ok(proc) => proc.branch,
        Err(_) if args.who.contains('@') && repo.has_branch(&args.who) => args.who.clone(),
        Err(e) => return Err(e),
    };
    let text = match &args.path {
        None => repo.git(&["ls-tree", "-r", "--name-only", &branch])?,
        Some(path) => repo
            .git(&["show", &format!("{branch}:{path}")])
            .map_err(|_| format!("{} has no file `{path}` (`jarvis show {}` lists its files)", args.who, args.who))?,
    };
    out.line(&text);
    Ok(())
}

// -------------------------------------------------------------------- log

/// Records are about a process when it did them, or when they bring it a message.
const ABOUT_PROC: &str = "(?2 IS NULL OR proc = ?2 OR (kind IN ('send', 'arrive') AND json_extract(data, '$.to') = ?2))";
/// A step is something an agent did inside a turn. There are many, so they
/// are shown only when asked for.
const STEPS: &str = "(?3 OR kind != 'step')";

/// Records after `after` (only those about `who`, if given), as lines for a
/// terminal. `last` is moved to the newest record seen.
pub fn fact_lines(conn: &Connection, me: &str, after: i64, before: Option<i64>, who: Option<&str>, steps: bool, last: &mut i64) -> Res<Vec<String>> {
    let label = |addr: &str| -> String { if !me.is_empty() && kernel_of(addr) == me { name_of(addr).to_string() } else { addr.to_string() } };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT seq, strftime('%H:%M:%S', at / 1000, 'unixepoch', 'localtime'), proc, kind, data, k
         FROM records WHERE seq > ?1 AND {ABOUT_PROC} AND {STEPS} AND (?4 IS NULL OR seq < ?4) ORDER BY seq"
    ))?;
    let rows: Vec<(i64, String, String, String, String, String)> = stmt
        .query_map(params![after, who, steps, before], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?
        .collect::<Result<_, _>>()?;
    let mut lines = Vec::new();
    for (seq, time, proc, kind, data, k) in rows {
        *last = seq;
        let d: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        let text = |key: &str| d[key].as_str().unwrap_or("").to_string();
        let what = match kind.as_str() {
            "send" => format!("→ {}: {}", label(&text("to")), clip(&text("body"), 140)),
            "arrive" => format!("← {} (through {}): {}", label(&text("from")), text("via"), clip(&text("body"), 120)),
            "forward" => format!("its message {} was passed on to {}", text("message"), text("next")),
            "undeliverable" => format!("the message {} could not be delivered: {}", text("message"), text("why")),
            "turn" => {
                let cost = d["cost_usd"].as_f64().map(|c| format!(" (${c:.3})")).unwrap_or_default();
                format!("turn{cost}: {}", clip(&text("reply"), 140))
            }
            "turn_start" => format!("turn started on {} message(s)", d["messages"].as_array().map_or(0, Vec::len)),
            "turn_failed" => format!("TURN FAILED ({} in a row, next try in {}s): {}", d["failures"], d["retry_in_s"], clip(&text("error"), 200)),
            "turn_interrupted" => "turn interrupted; its mail waits".to_string(),
            "turn_end" => "turn ended".to_string(),
            "spawn" if d["continued"] == true => format!("carried on here by {}", label(&text("parent"))),
            "spawn" => format!("created by {}", label(&text("parent"))),
            "stop" => format!("stopped by {}", label(&text("by"))),
            "start" => format!("started by {}", label(&text("by"))),
            "tag" => {
                let set = d["set"].as_object().map(|set| set.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
                format!("tagged by {}: {}", label(&text("by")), set.unwrap_or_default())
            }
            "remove" => {
                let cost = d["cost_usd"].as_f64().map(|c| format!(", had cost ${c:.3}")).unwrap_or_default();
                format!("removed by {} (was at {}{cost})", label(&text("by")), text("commit"))
            }
            "commit" => format!("commit {} {}", text("commit").chars().take(8).collect::<String>(), text("subject")),
            "commit_skipped" => format!("nothing committed: {}", text("why")),
            "handled_by_code" => format!("its handler dealt with {} message(s)", d["messages"].as_array().map_or(0, Vec::len)),
            "run_start" => "service started (.jarvis/run)".to_string(),
            "run_exit" => format!("service exited (.jarvis/run): {}", text("status")),
            "kernel_error" => format!("KERNEL PROBLEM: {}", clip(&text("error"), 200)),
            "kernel" => "a new kernel".to_string(),
            "up" => "is up".to_string(),
            "closed" if text("kernel").is_empty() => "closed".to_string(),
            "closed" => format!("{} is taken to be gone (said {})", text("kernel"), label(&text("by"))),
            "link_up" => format!("linked to {}", text("kernel")),
            "link_down" => format!("the link to {} went down: {}", text("kernel"), clip(&text("why"), 120)),
            "link_refused" => format!("a link was refused: {}", clip(&text("why"), 160)),
            "step" => format!("  · {:<8} {}", text("step"), clip(&text("text"), 160)),
            other if d == json!({}) => other.to_string(),
            other => format!("{other} {}", clip(&data, 160)),
        };
        // Who it is about; for what a kernel says of itself, that kernel.
        let about = if name_of(&proc) == KERNEL { format!("[{k}]") } else { label(&proc) };
        lines.push(format!("{time} {about:<20} {what}"));
    }
    Ok(lines)
}

fn log(ctx: &Ctx, args: cli::Log, out: &mut dyn Out) -> Res<()> {
    let conn = &ctx.conn;
    let who = match &args.who {
        Some(who) => Some(address(ctx, who)?),
        None => None,
    };
    let who = who.as_deref();
    let count = args.count.unwrap_or(if args.json { 500 } else { 30 });
    let follower = if args.follow { Some(ctx.follow()?) } else { None };
    // Start just before the oldest of the newest `count` records.
    let window: i64 = conn.query_row(
        &format!("SELECT COALESCE(MIN(seq), 0) - 1 FROM (SELECT seq FROM records WHERE seq > ?1 AND {ABOUT_PROC} AND {STEPS} AND (?5 IS NULL OR seq < ?5) ORDER BY seq DESC LIMIT ?4)"),
        params![args.after.unwrap_or(0), who, args.steps, count, args.before],
        |r| r.get(0),
    )?;
    let mut last = window.max(args.after.unwrap_or(0));
    if args.json {
        let mut stmt = conn.prepare(&format!("SELECT seq, at, proc, kind, data, k, n FROM records WHERE seq > ?1 AND {ABOUT_PROC} AND {STEPS} AND (?4 IS NULL OR seq < ?4) ORDER BY seq"))?;
        let rows: Vec<Value> = stmt.query_map(params![last, who, args.steps, args.before], fact_row)?.collect::<Result<_, _>>()?;
        out.line(&Value::Array(rows).to_string());
        return Ok(());
    }
    let mut printed = false;
    loop {
        let after = last;
        for line in fact_lines(conn, &ctx.me, after, args.before, who, args.steps, &mut last)? {
            if !out.line(&line) {
                return Ok(());
            }
            printed = true;
        }
        match &follower {
            Some(news) => {
                if news.recv().is_err() {
                    return Ok(());
                }
            }
            None => break,
        }
    }
    if !printed {
        out.line("nothing has happened yet");
    }
    Ok(())
}

fn fact_row(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let data: String = r.get(4)?;
    Ok(json!({
        "id": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "proc": r.get::<_, String>(2)?,
        "kind": r.get::<_, String>(3)?, "data": serde_json::from_str::<Value>(&data).unwrap_or(Value::Null),
        "kernel": r.get::<_, String>(5)?, "n": r.get::<_, Option<i64>>(6)?,
    }))
}

// ------------------------------------------------------------------ watch

/// Print every change as a JSON line, for as long as someone is reading.
fn watch(ctx: &Ctx, args: cli::Watch, out: &mut dyn Out) -> Res<()> {
    let conn = &ctx.conn;
    let news = ctx.follow()?;
    let mut cursor: i64 = match args.after {
        Some(after) => after,
        None => conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM records", [], |r| r.get(0))?,
    };
    // What was last said about each process.
    let mut said: HashMap<String, String> = HashMap::new();
    // The messages not dealt with yet, and their status when last seen.
    let mut open: HashMap<String, (String, i64)> = HashMap::new();
    let mut first = true;

    loop {
        let mut touched: BTreeSet<String> = BTreeSet::new();

        // New facts, in order. (The first look only says how things stand.)
        while !first {
            let mut stmt = conn.prepare_cached(&format!(
                "SELECT seq, at, proc, kind, data, k, n FROM records WHERE seq > ?1 AND (?2 OR kind != 'step') ORDER BY seq LIMIT 2000"
            ))?;
            let facts: Vec<Value> = stmt.query_map(params![cursor, args.steps], fact_row)?.collect::<Result<_, _>>()?;
            let more = facts.len() == 2000;
            for fact in facts {
                cursor = fact["id"].as_i64().unwrap_or(cursor);
                let kind = fact["kind"].as_str().unwrap_or("").to_string();
                let mut line = json!({"kind": "fact", "fact": fact});
                if kind == "send" || kind == "arrive" {
                    if let Some(to) = line["fact"]["data"]["to"].as_str() {
                        touched.insert(to.to_string());
                    }
                    line["message"] = line["fact"]["data"]["id"].as_str().and_then(|id| message_json(conn, id)).unwrap_or(Value::Null);
                }
                if kind != "step" {
                    touched.insert(line["fact"]["proc"].as_str().unwrap_or("").to_string());
                }
                // What a kernel says of itself can change how all its processes stand.
                if matches!(kind.as_str(), "up" | "closed") {
                    touched.insert("*".into());
                }
                if !out.line(&line.to_string()) {
                    return Ok(());
                }
            }
            if !more {
                break;
            }
        }

        // Messages whose status changed.
        let mut stmt = conn.prepare_cached(&format!("SELECT id, status, attempts FROM messages WHERE {OPEN}"))?;
        let waiting: HashMap<String, (String, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect::<Result<_, _>>()?;
        let mut changed: Vec<String> = waiting.iter().filter(|(id, state)| open.get(*id) != Some(*state)).map(|(id, _)| id.clone()).collect();
        changed.extend(open.keys().filter(|id| !waiting.contains_key(*id)).cloned());
        open = waiting;
        if !first {
            let mut changed: Vec<Value> = changed.iter().filter_map(|id| message_json(conn, id)).collect();
            changed.sort_by_key(|m| m["at"].as_i64());
            for message in changed {
                for who in ["from", "to"] {
                    touched.insert(message[who].as_str().unwrap_or("").to_string());
                }
                if !out.line(&json!({"kind": "message", "message": message}).to_string()) {
                    return Ok(());
                }
            }
        }

        // Processes: at first all of them, then those something happened to.
        let procs = index::procs(conn, false)?;
        for p in &procs {
            if !first && !touched.contains(&p.addr) && !touched.contains("*") && said.contains_key(&p.addr) {
                continue;
            }
            let described = describe(ctx, p)?.to_string();
            if said.get(&p.addr) == Some(&described) {
                continue;
            }
            let line = format!("{{\"kind\":\"process\",\"process\":{described}}}");
            said.insert(p.addr.clone(), described);
            if !out.line(&line) {
                return Ok(());
            }
        }
        let gone: Vec<String> = said.keys().filter(|addr| !procs.iter().any(|p| &p.addr == *addr)).cloned().collect();
        for addr in gone {
            said.remove(&addr);
            if !out.line(&json!({"kind": "gone", "id": addr}).to_string()) {
                return Ok(());
            }
        }
        if first {
            first = false;
            if !out.line(&json!({"kind": "ready", "cursor": cursor}).to_string()) {
                return Ok(());
            }
            // The facts above the starting point, if any, are next.
            continue;
        }
        if news.recv().is_err() {
            return Ok(());
        }
    }
}

/// Used by `jarvis ... --store DIR`: which commands can be answered from a
/// store alone.
pub fn reads_only(command: &Cmd) -> bool {
    match command {
        Cmd::Ps(_) | Cmd::Messages(_) | Cmd::Show(_) => true,
        Cmd::Log(args) => !args.follow,
        Cmd::Kernels(args) => args.close.is_none(),
        _ => false,
    }
}
