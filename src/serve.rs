//! Carrying out `jarvis` commands.
//!
//! A command typed anywhere is sent, as it was typed, to a kernel's socket.
//! A thread here parses it (with the same definitions as the command line,
//! `cli.rs`) and does what it says:
//!
//! - **Reading** this kernel's own state is done right here, on the thread's
//!   own connection to the kernel's database.
//! - **Changing** something on this kernel is handed to the kernel's loop as
//!   a `Mutation`, because the loop is the only writer.
//! - **Changing something on another kernel** is a request: it is sent there
//!   like a message, and this command waits a while for what came of it.
//! - **Asking another kernel** (which processes, what happened, what was
//!   said) sends the same command there over the links and puts the answers
//!   together. A kernel that does not answer is named, never left out
//!   silently.
//! - **Following** other kernels (`watch`, `log -f`) has each of them run
//!   the command and send its lines back as they come.
//!
//! No kernel holds another kernel's state, so every answer is as that kernel
//! gives it, at that moment.

use crate::children;
use crate::cli::{self, Cli, Cmd};
use crate::index::{self, HUMAN, KERNEL, Proc, Res, at, kernel_of, name_of};
use crate::kernel::{self, Config, Event, Mutation, Shared};
use crate::net;
use clap::Parser;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

/// How long a command waits to hear what came of a request to another kernel.
const REQUEST_PATIENCE: Duration = Duration::from_secs(20);
/// The exit code of a command that did only part of what was asked: a kernel
/// did not answer, or a request has not been carried out yet.
const PARTLY: i32 = 2;

/// Where a command's output goes.
pub trait Out {
    /// A line of output. False when nobody is reading any more.
    fn line(&mut self, text: &str) -> bool;
    /// Something for standard error.
    fn note(&mut self, text: &str);
    /// False when whoever was reading is known to be gone.
    fn alive(&self) -> bool {
        true
    }
}

/// Back over the socket, as it is produced.
struct ToSocket(UnixStream);

impl Out for ToSocket {
    fn line(&mut self, text: &str) -> bool {
        writeln!(self.0, "{}", json!({"o": text})).is_ok()
    }
    fn note(&mut self, text: &str) {
        let _ = writeln!(self.0, "{}", json!({"e": text}));
    }
}

/// Into a list: for a command carried out on behalf of another kernel.
#[derive(Default)]
struct Collected {
    lines: Vec<String>,
    notes: Vec<String>,
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

/// To a kernel that follows this one, line by line.
struct ToFollower {
    tx: Sender<Event>,
    sid: String,
    to: String,
    alive: Arc<AtomicBool>,
    notes: Vec<String>,
}

impl Out for ToFollower {
    fn line(&mut self, text: &str) -> bool {
        self.alive() && self.tx.send(Event::Publish { sid: self.sid.clone(), to: self.to.clone(), item: json!({"line": text}) }).is_ok()
    }
    fn note(&mut self, text: &str) {
        self.notes.push(text.to_string());
    }
    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

/// What a command is carried out with.
pub struct Ctx {
    /// This kernel's name.
    pub me: String,
    pub conn: Connection,
    pub cfg: Arc<Config>,
    tx: Sender<Event>,
    shared: Arc<Shared>,
}

impl Ctx {
    fn of_kernel(cfg: Arc<Config>, tx: Sender<Event>, shared: Arc<Shared>) -> Res<Ctx> {
        Ok(Ctx { me: cfg.name.clone(), conn: index::open(&cfg.db_path())?, cfg, tx, shared })
    }

    /// The same again, for another thread.
    fn again(&self) -> Res<Ctx> {
        Ctx::of_kernel(self.cfg.clone(), self.tx.clone(), self.shared.clone())
    }

    /// Have the kernel do something, and wait for its answer.
    fn mutate(&self, mutation: Mutation) -> Res<Value> {
        let (reply, answer) = mpsc::channel();
        self.tx.send(Event::Mutate(mutation, reply)).map_err(|_| "the kernel is stopping")?;
        Ok(answer.recv().map_err(|_| "the kernel stopped before it answered")??)
    }

    /// Be told whenever there are new records, or the kernels in reach changed.
    fn follow(&self) -> Receiver<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.shared.followers.lock().unwrap().push(tx);
        rx
    }

    /// Every other kernel this one has heard of and that nobody here took to
    /// be gone: those that can be reached now first, then the others. The
    /// others are asked too, so that they are named as not answering
    /// instead of being left out silently.
    fn known(&self) -> Res<Vec<String>> {
        let mut kernels = self.reachable();
        let mut stmt = self.conn.prepare("SELECT name FROM kernels WHERE closed = 0 ORDER BY name")?;
        for name in stmt.query_map([], |r| r.get::<_, String>(0))? {
            let name = name?;
            if name != self.me && !kernels.contains(&name) {
                kernels.push(name);
            }
        }
        Ok(kernels)
    }

    /// The kernels that can be reached over links now.
    fn reachable(&self) -> Vec<String> {
        self.shared.net.lock().unwrap().reachable.iter().map(|r| r.name.clone()).collect()
    }
}

/// Take the kernel's socket and answer what arrives on it, from now on.
/// Fails if a kernel of this name is already running on this machine.
pub fn listen(cfg: Arc<Config>, tx: Sender<Event>, shared: Arc<Shared>) -> Res<()> {
    let path = cfg.run_dir.join(format!("{}.sock", cfg.name));
    if UnixStream::connect(&path).is_ok() {
        return Err(format!("a kernel called `{}` is already running on this machine (its socket is {})", cfg.name, path.display()).into());
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

/// Carry out a command another kernel sent (a question, or a request), and
/// return what it printed.
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

/// Carry out a command for a kernel that follows this one: each line goes
/// back as it is printed, until the command ends or `alive` is switched off.
pub fn publish(cfg: Arc<Config>, tx: Sender<Event>, shared: Arc<Shared>, request: Value, sid: String, to: String, alive: Arc<AtomicBool>) {
    let mut out = ToFollower { tx: tx.clone(), sid: sid.clone(), to: to.clone(), alive, notes: Vec::new() };
    let code = match Ctx::of_kernel(cfg, tx.clone(), shared) {
        Ok(ctx) => handle(&ctx, &request, &mut out),
        Err(e) => {
            out.note(&format!("jarvis: {e}"));
            1
        }
    };
    let _ = tx.send(Event::Publish { sid, to, item: json!({"end": code, "why": out.notes.join("; ")}) });
}

/// Carry out one command. The request is `{"argv": what was typed after
/// `jarvis`, "by": who typed it, "stdin": what was piped in, "files": the
/// content of files it names}`. Returns the exit code.
fn handle(ctx: &Ctx, request: &Value, out: &mut dyn Out) -> i32 {
    match run(ctx, request, out) {
        Ok(code) => code,
        Err(e) => {
            out.note(&format!("jarvis: {e}"));
            1
        }
    }
}

/// Print what the kernel answered: `{"o": [lines], "e": [notes]}`.
fn print(out: &mut dyn Out, answer: &Value) {
    for line in answer["o"].as_array().into_iter().flatten() {
        out.line(line.as_str().unwrap_or(""));
    }
    for note in answer["e"].as_array().into_iter().flatten() {
        let note = note.as_str().unwrap_or("");
        out.note(&if note.starts_with("jarvis:") || note.starts_with("note:") { note.to_string() } else { format!("note: {note}") });
    }
}

fn words(list: &Value) -> Vec<String> {
    list.as_array().map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn run(ctx: &Ctx, request: &Value, out: &mut dyn Out) -> Res<i32> {
    let argv = words(&request["argv"]);
    let cli = Cli::try_parse_from(std::iter::once("jarvis".to_string()).chain(argv.clone())).map_err(|e| e.to_string())?;
    let command = cli.command.ok_or("no command was given")?;
    // Who is asking: a process, or the person at this kernel.
    let by = match request["by"].as_str().unwrap_or("") {
        address if address.contains('@') => address.to_string(),
        _ => at(HUMAN, &ctx.me),
    };
    // What a request to another kernel must carry along.
    let extra = json!({"stdin": request["stdin"], "files": request["files"]});

    match command {
        Cmd::Up(_) => Err("`jarvis up` starts a kernel; it is not something a kernel is asked to do".into()),
        Cmd::Spawn(args) => {
            let (_, kernel) = index::address(&args.address, "process")?;
            if kernel != ctx.me {
                return request_of(ctx, out, kernel, &by, argv, extra);
            }
            let prompt = match (&args.prompt, &args.prompt_file) {
                (Some(text), _) => Some(text.clone()),
                (None, Some(file)) => Some(request["files"][file].as_str().ok_or_else(|| format!("could not read {file}"))?.to_string()),
                (None, None) => None,
            };
            print(out, &ctx.mutate(Mutation::Spawn { args, by, prompt })?);
            Ok(0)
        }
        Cmd::Ps(args) => ps(ctx, &by, args, out),
        Cmd::Stop(args) => each_kernel(ctx, out, &by, "stop", &args.who, &[], |who| Mutation::SetStatus { who, alive: false, by: by.clone() }),
        Cmd::Start(args) => each_kernel(ctx, out, &by, "start", &args.who, &[], |who| Mutation::SetStatus { who, alive: true, by: by.clone() }),
        Cmd::Remove(args) => {
            let flags: &[&str] = if args.keep_branch { &["--keep-branch"] } else { &[] };
            let mut worst = 0;
            // One at a time: each is answered once nothing of the process runs any more.
            for who in &args.who {
                let one = std::slice::from_ref(who);
                worst = worst.max(each_kernel(ctx, out, &by, "remove", one, flags, |mut who| Mutation::Remove { who: who.remove(0), keep_branch: args.keep_branch, by: by.clone() })?);
            }
            Ok(worst)
        }
        Cmd::Tag(args) => {
            let (_, kernel) = index::address(&args.who, "process")?;
            let tags: Vec<(String, String)> = args.tags.iter().map(|t| parse_tag(t)).collect::<Res<_>>()?;
            if kernel != ctx.me {
                return if tags.is_empty() { relay(ctx, out, kernel, &by, argv) } else { request_of(ctx, out, kernel, &by, argv, extra) };
            }
            if !tags.is_empty() {
                ctx.mutate(Mutation::Tag { who: args.who.clone(), tags, by })?;
            }
            let proc = local(ctx, &args.who)?;
            for (key, value) in tags_of(&ctx.conn, &proc)?.as_object().into_iter().flatten() {
                out.line(&format!("{key}={}", value.as_str().unwrap_or("")));
            }
            Ok(0)
        }
        Cmd::Send(args) => {
            let mut body = args.text.join(" ");
            if body == "-" || body.is_empty() {
                body = request["stdin"].as_str().unwrap_or("").to_string();
            }
            if body.trim().is_empty() {
                return Err("there was nothing to send: the message was empty".into());
            }
            let from = match args.sender {
                None => by,
                Some(who) => {
                    index::address(&who, "sender")?;
                    who
                }
            };
            let sent = ctx.mutate(Mutation::Send { from, to: args.to, body: body.trim_end().to_string() })?;
            if args.json {
                out.line(&json!({"id": sent["id"], "to": sent["to"], "note": sent["e"][0]}).to_string());
            } else {
                print(out, &sent);
            }
            Ok(0)
        }
        Cmd::Messages(args) => messages(ctx, &by, args, out),
        Cmd::Show(args) => show(ctx, &by, args, argv, out),
        Cmd::Log(args) => log(ctx, &by, args, out),
        Cmd::Watch(args) => watch(ctx, &by, args, out),
        Cmd::Kernels(args) => {
            match args.close {
                Some(name) => print(out, &ctx.mutate(Mutation::CloseKernel { name, by })?),
                None => kernels(ctx, args.json, out)?,
            }
            Ok(0)
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
                None if args.remove => return Err("say which link to forget, as it was made (`jarvis link` lists them)".into()),
                None => links(ctx, args.json, out)?,
            }
            Ok(0)
        }
    }
}

// ----------------------------------------------------------- other kernels

/// Have another kernel carry out a command, and wait a while for what came
/// of it. The request travels like a message: if that kernel cannot be
/// reached now, it is carried out when it can.
fn request_of(ctx: &Ctx, out: &mut dyn Out, kernel: &str, by: &str, argv: Vec<String>, extra: Value) -> Res<i32> {
    let (outcome, came) = mpsc::channel();
    let accepted = ctx.mutate(Mutation::Request { kernel: kernel.to_string(), by: by.to_string(), argv, extra, outcome })?;
    let id = accepted["o"][0].as_str().unwrap_or("").to_string();
    // A kernel that cannot be reached will not answer soon: say so at once.
    let patience = if accepted["reachable"] == true { REQUEST_PATIENCE } else { Duration::ZERO };
    match came.recv_timeout(patience) {
        Ok(result) => {
            print(out, &result);
            Ok(result["code"].as_i64().unwrap_or(1) as i32)
        }
        Err(_) => {
            let why = if accepted["reachable"] == true {
                format!("has not answered in {} seconds", REQUEST_PATIENCE.as_secs())
            } else {
                "cannot be reached right now (`jarvis link` shows why a link is down)".to_string()
            };
            out.note(&format!(
                "jarvis: not done yet: the kernel `{kernel}` {why}. The request {id} waits and is carried out when it gets there. \
                 You are sent a message with what came of it, and `jarvis messages` shows it too"
            ));
            Ok(PARTLY)
        }
    }
}

/// Begin asking another kernel to carry out a command and say what it printed.
fn ask(ctx: &Ctx, kernel: &str, by: &str, argv: Vec<String>) -> Receiver<Result<Value, String>> {
    let (reply, answer) = mpsc::channel();
    let request = json!({"argv": argv, "by": by});
    if ctx.tx.send(Event::Mutate(Mutation::Ask { on: kernel.to_string(), request }, reply.clone())).is_err() {
        let _ = reply.send(Err("the kernel here is stopping".into()));
    }
    answer
}

/// Ask another kernel to carry out a command, and print what it printed.
fn relay(ctx: &Ctx, out: &mut dyn Out, kernel: &str, by: &str, argv: Vec<String>) -> Res<i32> {
    match ask(ctx, kernel, by, argv).recv() {
        Ok(Ok(answer)) => {
            print(out, &answer);
            Ok(answer["code"].as_i64().unwrap_or(1) as i32)
        }
        // Not an answer about what was asked: the kernel could not be asked.
        Ok(Err(why)) => Ok(report(out, &[(kernel.to_string(), why)])),
        Err(_) => Err("the kernel here stopped before the answer came".into()),
    }
}

/// Do something to processes that may be on several kernels: on this kernel
/// directly, on each other kernel by a request.
fn each_kernel(ctx: &Ctx, out: &mut dyn Out, by: &str, command: &str, who: &[String], flags: &[&str], here: impl Fn(Vec<String>) -> Mutation) -> Res<i32> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for address in who {
        let (_, kernel) = index::address(address, "process")?;
        match groups.iter_mut().find(|(k, _)| k == kernel) {
            Some((_, list)) => list.push(address.clone()),
            None => groups.push((kernel.to_string(), vec![address.clone()])),
        }
    }
    let mut worst = 0;
    for (kernel, addresses) in groups {
        let code = if kernel == ctx.me {
            print(out, &ctx.mutate(here(addresses))?);
            0
        } else {
            let argv: Vec<String> = std::iter::once(command.to_string()).chain(addresses).chain(flags.iter().map(|f| f.to_string())).collect();
            request_of(ctx, out, &kernel, by, argv, json!({}))?
        };
        worst = worst.max(code);
    }
    Ok(worst)
}

/// What a command looks at: kernels, and for each either all of it (no
/// processes named) or only the processes named.
type Scope = Vec<(String, Vec<String>)>;

/// Kernels that gave no answer, and why.
type Silent = Vec<(String, String)>;

/// Work out what a command looks at from what was typed: nothing means this
/// kernel, `@K` a kernel, `NAME@K` a process, `--all` every kernel in reach.
fn scope(ctx: &Ctx, what: &[String], all: bool) -> Res<Scope> {
    if all {
        return Ok(std::iter::once(ctx.me.clone()).chain(ctx.known()?).map(|k| (k, Vec::new())).collect());
    }
    if what.is_empty() {
        return Ok(vec![(ctx.me.clone(), Vec::new())]);
    }
    let mut whole: BTreeSet<String> = BTreeSet::new();
    let mut some: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in what {
        match item.strip_prefix('@') {
            Some(kernel) if !kernel.is_empty() => {
                whole.insert(kernel.to_string());
            }
            _ => {
                let (_, kernel) = index::address(item, "process")?;
                some.entry(kernel.to_string()).or_default().push(item.clone());
            }
        }
    }
    let mut scope: Scope = whole.iter().map(|k| (k.clone(), Vec::new())).collect();
    scope.extend(some.into_iter().filter(|(k, _)| !whole.contains(k)));
    Ok(scope)
}

/// One part of a scope as it is typed: `@K`, or the processes.
fn typed(kernel: &str, procs: &[String]) -> Vec<String> {
    if procs.is_empty() { vec![format!("@{kernel}")] } else { procs.to_vec() }
}

/// Rows from every kernel in the scope: this kernel's are worked out here by
/// `here`, every other kernel is asked for `command ... --json`. Returns the
/// rows, and for each kernel that gave none, why.
fn gather(ctx: &Ctx, by: &str, scope: &Scope, command: &str, flags: &[String], here: impl Fn(&[String]) -> Res<Vec<Value>>) -> Res<(Vec<Value>, Silent)> {
    let mut rows = Vec::new();
    let mut asked = Vec::new();
    for (kernel, procs) in scope {
        if *kernel == ctx.me {
            rows.extend(here(procs)?);
        } else {
            let argv: Vec<String> = std::iter::once(command.to_string()).chain(typed(kernel, procs)).chain(flags.iter().cloned()).chain(std::iter::once("--json".to_string())).collect();
            asked.push((kernel.clone(), ask(ctx, kernel, by, argv)));
        }
    }
    let mut silent = Vec::new();
    for (kernel, answer) in asked {
        let got: Result<Vec<Value>, String> = match answer.recv() {
            Ok(Ok(answer)) if answer["code"] == 0 => answer["o"][0].as_str().and_then(|text| serde_json::from_str::<Vec<Value>>(text).ok()).ok_or_else(|| "its answer could not be read (is it another version of jarvis? `jarvis kernels` shows)".to_string()),
            Ok(Ok(answer)) => Err(words(&answer["e"]).join("; ")),
            Ok(Err(why)) => Err(why),
            Err(_) => Err("the kernel here stopped before the answer came".to_string()),
        };
        match got {
            Ok(mut more) => {
                for row in &mut more {
                    if row.get("kernel").is_none() {
                        row["kernel"] = json!(kernel);
                    }
                }
                rows.extend(more);
            }
            Err(why) => silent.push((kernel, why)),
        }
    }
    Ok((rows, silent))
}

/// Name the kernels that gave no answer. Returns the command's exit code.
fn report(out: &mut dyn Out, silent: &[(String, String)]) -> i32 {
    for (kernel, why) in silent {
        out.note(&format!("jarvis: nothing from the kernel `{kernel}`: {}", why.trim_start_matches("jarvis: ")));
    }
    if silent.is_empty() { 0 } else { PARTLY }
}

// ---------------------------------------------------------------- helpers

/// A process of this kernel, from its address.
fn local(ctx: &Ctx, who: &str) -> Res<Proc> {
    index::address(who, "process")?;
    index::find_proc(&ctx.conn, who)?.ok_or_else(|| format!("there is no process `{who}` (`jarvis ps @{}` lists the processes of this kernel)", ctx.me).into())
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

/// A moment (in milliseconds) as the time of day here: `14:03:59`.
fn clock(ms: i64) -> String {
    let seconds = (ms / 1000) as libc::time_t;
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut parts) };
    format!("{:02}:{:02}:{:02}", parts.tm_hour, parts.tm_min, parts.tm_sec)
}

/// The first line of real text in a process's `.jarvis/ABOUT.md`: how it
/// describes itself.
fn about(ctx: &Ctx, p: &Proc) -> String {
    let text = fs::read_to_string(ctx.cfg.proc_dir(&p.name).join(".jarvis/ABOUT.md")).unwrap_or_default();
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
        ("failed", _) => "could not be carried out".into(),
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

/// Everything `ps` says about one process of this kernel.
fn describe(ctx: &Ctx, p: &Proc) -> Res<Value> {
    let conn = &ctx.conn;
    let dir = ctx.cfg.proc_dir(&p.name);
    // What its agent has cost so far, as far as its harness reported.
    let (turns, cost): (i64, Option<f64>) = conn
        .prepare_cached("SELECT COUNT(*), SUM(json_extract(data, '$.cost_usd')) FROM records WHERE proc = ?1 AND kind = 'turn'")?
        .query_row([&p.addr], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let mail: i64 = conn
        .prepare_cached("SELECT COUNT(*) FROM messages WHERE recipient = ?1 AND status IN ('pending', 'taken')")?
        .query_row([&p.addr], |r| r.get(0))?;
    // `failing`: its last turn failed, and its mail is waiting for the next try.
    let state = if p.status != "alive" {
        "stopped"
    } else if p.busy {
        "thinking"
    } else if p.failures > 0 {
        "failing"
    } else {
        "idle"
    };
    Ok(json!({
        "id": p.addr, "name": p.name, "kernel": p.kernel, "state": state, "about": about(ctx, p),
        "from": p.from, "tags": tags_of(conn, p)?, "branch": p.branch,
        "directory": dir.display().to_string(), "harness": p.harness,
        "parent": p.parent, "created_at": p.created_at,
        "turns": turns, "cost_usd": cost, "mail_waiting": mail,
        "busy_since": p.busy_since, "failures": p.failures,
        "has_run": kernel::is_executable(&dir.join(".jarvis/run")),
        "has_handle": kernel::is_executable(&dir.join(".jarvis/handle")),
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

/// This kernel's processes: all of them, or only those named.
fn ps_rows(ctx: &Ctx, only: &[String]) -> Res<Vec<Value>> {
    index::procs(&ctx.conn)?.iter().filter(|p| only.is_empty() || only.contains(&p.addr)).map(|p| describe(ctx, p)).collect()
}

fn ps(ctx: &Ctx, by: &str, args: cli::Ps, out: &mut dyn Out) -> Res<i32> {
    // One process: everything about it, from its own kernel.
    if let [one] = args.what.as_slice()
        && !one.starts_with('@') {
            let (_, kernel) = index::address(one, "process")?;
            if kernel != ctx.me {
                let argv = ["ps", one.as_str()].iter().map(|w| w.to_string()).chain(args.json.then(|| "--json".to_string())).collect();
                return relay(ctx, out, kernel, by, argv);
            }
            ps_one(ctx, &local(ctx, one)?, args.json, out)?;
            return Ok(0);
        }
    let scope = scope(ctx, &args.what, args.all)?;
    let (rows, silent) = gather(ctx, by, &scope, "ps", &[], |only| ps_rows(ctx, only))?;
    if args.json {
        out.line(&Value::Array(rows).to_string());
        return Ok(report(out, &silent));
    }
    if rows.is_empty() {
        // Said only when it is known: not when a kernel gave no answer.
        if silent.is_empty() {
            out.line("no processes there. `jarvis help spawn` shows how to create one.");
        }
    } else {
        let wide = rows.iter().map(|r| r["id"].as_str().unwrap_or("").len()).max().unwrap_or(0).max(8);
        out.line(&format!("{:<wide$}  {:<13} {:<20} ABOUT", "PROCESS", "STATE", "MADE BY"));
        for r in &rows {
            out.line(&format!("{:<wide$}  {:<13} {:<20} {}", r["id"].as_str().unwrap_or("-"), state_words(r), r["parent"].as_str().unwrap_or(""), r["about"].as_str().unwrap_or("")));
        }
    }
    Ok(report(out, &silent))
}

/// Everything about one process of this kernel.
fn ps_one(ctx: &Ctx, p: &Proc, as_json: bool, out: &mut dyn Out) -> Res<()> {
    let conn = &ctx.conn;
    let mut row = describe(ctx, p)?;
    let service_pid: Option<u32> = conn
        .query_row("SELECT pid, started FROM children WHERE proc = ?1 AND kind = 'run'", [&p.name], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)))
        .ok()
        .filter(|(pid, since)| children::is_running(*pid, since))
        .map(|(pid, _)| pid);
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
    let head = ctx.cfg.repo.git(&["rev-parse", "--short", &p.branch]).ok();
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
    let no_agent = p.harness == kernel::NO_AGENT;
    line("address", p.addr.clone());
    line("state", state_words(&row));
    line("about", match row["about"].as_str() {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => "(has not described itself yet)".into(),
    });
    line("made by", format!("{}, {} ago", p.parent, ago(p.created_at)));
    if let Some(from) = &p.from {
        line("from", from.clone());
    }
    line("agent", if no_agent { "none: its code deals with its messages".into() } else { format!("harness `{}`", p.harness) });
    if let Some(tags) = row["tags"].as_object() {
        line("tags", tags.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
    }
    line("branch", format!("{}{}", p.branch, head.map(|h| format!(" (at {h})")).unwrap_or_default()));
    line("directory", row["directory"].as_str().unwrap_or("").to_string());
    line("service", match (row["has_run"].as_bool(), service_pid, &last_exit) {
        (Some(true), Some(pid), _) => format!("running (pid {pid})"),
        (Some(true), None, Some((at, _, data))) => format!("not running; it exited {} ago with {}", ago(*at), data["status"].as_str().unwrap_or("?")),
        (Some(true), None, None) => "not running".into(),
        _ => "none (no .jarvis/run)".into(),
    });
    line("handler", if row["has_handle"] == true { "yes (.jarvis/handle)".into() } else { "none".into() });
    if !no_agent {
        line("turns", match row["cost_usd"].as_f64() {
            Some(cost) => format!("{}, costing ${cost:.2} in all", row["turns"]),
            None => format!("{}", row["turns"]),
        });
    }
    line("mail", format!("{} waiting", row["mail_waiting"]));
    match last_turn {
        None if no_agent => {}
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
    let net = ctx.shared.net.lock().unwrap().clone();
    // A kernel is stopped by a signal to its process (TERM, or Ctrl-C where it runs).
    let pid = std::process::id();
    let mut rows = vec![json!({"name": ctx.me, "state": format!("here (process {pid})"), "here": true, "reachable": true, "hops": 0, "via": Value::Null,
                               "version": cli::VERSION, "closed": false, "last_reached_at": Value::Null, "pid": pid})];
    for reach in &net.reachable {
        let state = match reach.hops {
            1 => "linked".to_string(),
            hops => format!("reachable through {} ({hops} links away)", reach.via),
        };
        rows.push(json!({"name": reach.name, "state": state, "here": false, "reachable": true, "hops": reach.hops, "via": reach.via,
                         "version": reach.version, "closed": false, "last_reached_at": index::now_ms()}));
    }
    // Those that were reached before and cannot be now.
    let mut stmt = ctx.conn.prepare("SELECT name, version, closed, closed_by, last_reached FROM kernels ORDER BY name")?;
    let mut known = stmt.query([])?;
    while let Some(row) = known.next()? {
        let (name, version, closed, closed_by, last_reached): (String, Option<String>, bool, Option<String>, i64) = (row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0, row.get(3)?, row.get(4)?);
        if name == ctx.me || net.reachable.iter().any(|r| r.name == name) {
            continue;
        }
        let state = match (closed, closed_by) {
            (true, Some(by)) => format!("taken to be gone (said {by})"),
            (true, None) => "taken to be gone".to_string(),
            (false, _) if last_reached > 0 => format!("not reachable; last reached {} ago", ago(last_reached)),
            (false, _) => "not reachable".to_string(),
        };
        rows.push(json!({"name": name, "state": state, "here": false, "reachable": false, "hops": Value::Null, "via": Value::Null,
                         "version": version, "closed": closed, "last_reached_at": last_reached}));
    }
    if as_json {
        out.line(&Value::Array(rows).to_string());
        return Ok(());
    }
    let wide = rows.iter().map(|r| r["name"].as_str().unwrap_or("").len()).max().unwrap_or(0).max(6);
    out.line(&format!("{:<wide$}  {:<24} STATE", "KERNEL", "JARVIS"));
    let mut differ = false;
    for r in &rows {
        // Kernels of different builds may not understand each other fully.
        let other = r["version"].as_str().is_some_and(|v| v != cli::VERSION);
        differ |= other && r["reachable"] == true;
        out.line(&format!("{:<wide$}  {:<24} {}", r["name"].as_str().unwrap_or(""), format!("{}{}", r["version"].as_str().unwrap_or("-"), if other { " *" } else { "" }), r["state"].as_str().unwrap_or("")));
    }
    if differ {
        out.line("* another build of jarvis than this kernel's. Kernels that talk to each other should be the same build.");
    }
    Ok(())
}

fn links(ctx: &Ctx, as_json: bool, out: &mut dyn Out) -> Res<()> {
    let net = ctx.shared.net.lock().unwrap().clone();
    let rows: Vec<Value> = net
        .links
        .iter()
        .map(|l| {
            let how = l.spec.as_deref().map(kernel::link_words).unwrap_or_else(|| "made from the other end".to_string());
            let state = match (&l.peer, &l.problem) {
                (Some(peer), _) => format!("up, to {peer}"),
                (None, Some(problem)) => {
                    let tried = match l.tries {
                        0 | 1 => String::new(),
                        n => format!("; tried {n} times"),
                    };
                    let next = match l.next_try_s {
                        Some(seconds) => format!("; next try in {seconds}s"),
                        None => "; trying now".to_string(),
                    };
                    format!("down: {problem}{tried}{next}")
                }
                (None, None) => "being made".to_string(),
            };
            json!({"link": how, "kernel": l.peer, "up": l.peer.is_some(), "state": state, "problem": l.problem, "tries": l.tries})
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

const OPEN: &str = "status NOT IN ('handled', 'dropped', 'undeliverable', 'failed')";

/// `seq`: the number of the fact (as `jarvis log --json` and `jarvis watch`
/// number them) that first brought this message to this kernel.
const MESSAGE_COLUMNS: &str = "id, at, sender, recipient, body, status, attempts, seq, result";

fn message_row(kernel: &str, r: &rusqlite::Row) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": r.get::<_, String>(0)?, "at": r.get::<_, i64>(1)?, "from": r.get::<_, String>(2)?,
        "to": r.get::<_, String>(3)?, "body": r.get::<_, String>(4)?, "status": r.get::<_, String>(5)?,
        "attempts": r.get::<_, i64>(6)?, "seq": r.get::<_, i64>(7)?, "result": r.get::<_, Option<String>>(8)?,
        "kernel": kernel,
    }))
}

fn message_json(ctx: &Ctx, id: &str) -> Option<Value> {
    ctx.conn.prepare_cached(&format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1")).ok()?.query_row([id], |r| message_row(&ctx.me, r)).ok()
}

/// How far along a status is, to choose between two kernels' word on one message.
fn rank(status: &str) -> u8 {
    match status {
        "sent" => 0,
        "forwarded" => 1,
        "pending" => 2,
        "taken" => 3,
        _ => 4,
    }
}

/// A moment given as a time in milliseconds, or as the name of a message
/// this kernel has: then the time that message was sent.
fn moment(ctx: &Ctx, when: &Option<String>) -> Res<Option<i64>> {
    let Some(when) = when else { return Ok(None) };
    if let Ok(time) = when.parse::<i64>() {
        return Ok(Some(time));
    }
    match ctx.conn.query_row("SELECT at FROM messages WHERE id = ?1", [when], |r| r.get(0)) {
        Ok(time) => Ok(Some(time)),
        Err(_) => Err(format!("the kernel `{}` has no message `{when}`. Give the moment as a time instead (`at` in --json), or ask the kernel that has the message", ctx.me).into()),
    }
}

/// This kernel's messages, newest last: all, or those to or from `who`.
/// `after` and `before` are times.
fn message_rows(ctx: &Ctx, who: Option<&str>, args: &cli::Messages, after: Option<i64>, before: Option<i64>, count: i64) -> Res<Vec<Value>> {
    let mut stmt = ctx.conn.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages
         WHERE (?1 IS NULL OR at > ?1)
           AND (?2 IS NULL OR at < ?2)
           AND (?3 = 0 OR {OPEN})
           AND (?4 IS NULL OR sender = ?4 OR recipient = ?4)
           AND (?5 IS NULL OR sender = ?5 OR recipient = ?5)
         ORDER BY at DESC, id DESC LIMIT ?6"
    ))?;
    let mut rows: Vec<Value> = stmt.query_map(params![after, before, args.waiting, who, args.with, count], |r| message_row(&ctx.me, r))?.collect::<Result<_, _>>()?;
    rows.reverse();
    Ok(rows)
}

/// Who has written to whom on this kernel, and how much.
fn pair_rows(ctx: &Ctx, who: Option<&str>) -> Res<Vec<Value>> {
    let mut stmt = ctx.conn.prepare(&format!(
        "SELECT MIN(sender, recipient), MAX(sender, recipient), COUNT(*), SUM({OPEN}),
                SUM(sender LIKE 'kernel@%'), MAX(at), MAX(seq)
         FROM messages WHERE sender != recipient AND (?1 IS NULL OR sender = ?1 OR recipient = ?1) GROUP BY 1, 2 ORDER BY 6"
    ))?;
    let rows = stmt
        .query_map([who], |r| {
            Ok(json!({
                "a": r.get::<_, String>(0)?, "b": r.get::<_, String>(1)?, "count": r.get::<_, i64>(2)?,
                "waiting": r.get::<_, i64>(3)?, "by_jarvis": r.get::<_, i64>(4)?, "last_at": r.get::<_, i64>(5)?,
                "last_seq": r.get::<_, i64>(6)?, "kernel": ctx.me,
            }))
        })?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

/// Which kernels a command with one optional WHAT looks at, and the process
/// it is about, if one: nothing means this kernel, `@K` that kernel, a
/// process its own kernel.
fn one_scope(ctx: &Ctx, what: &Option<String>, all: bool) -> Res<(Scope, Option<String>)> {
    match what {
        Some(item) if !item.starts_with('@') => {
            let (_, kernel) = index::address(item, "process")?;
            Ok((vec![(kernel.to_string(), vec![item.clone()])], Some(item.clone())))
        }
        Some(item) => Ok((scope(ctx, std::slice::from_ref(item), false)?, None)),
        None => Ok((scope(ctx, &[], all)?, None)),
    }
}

fn messages(ctx: &Ctx, by: &str, args: cli::Messages, out: &mut dyn Out) -> Res<i32> {
    if let Some(other) = &args.with {
        index::address(other, "process")?;
    }
    let (scope, who) = one_scope(ctx, &args.what, args.all)?;
    let count = args.count.unwrap_or(if args.json { 500 } else { 20 });
    // The same question, as it is put to another kernel.
    let mut flags: Vec<String> = vec!["-n".into(), count.to_string()];
    // Other kernels are given the moments as times: they need not have the message named.
    let (after, before) = (moment(ctx, &args.after)?, moment(ctx, &args.before)?);
    for (flag, value) in [("--with", args.with.clone()), ("--after", after.map(|t| t.to_string())), ("--before", before.map(|t| t.to_string()))] {
        if let Some(value) = value {
            flags.extend([flag.to_string(), value]);
        }
    }
    if args.waiting {
        flags.push("--waiting".into());
    }
    if args.pairs {
        flags.push("--pairs".into());
        let (rows, silent) = gather(ctx, by, &scope, "messages", &flags, |_| pair_rows(ctx, who.as_deref()))?;
        // Two kernels that both know a pair say the same of it: one row each.
        let mut pairs: BTreeMap<(String, String), Value> = BTreeMap::new();
        for row in rows {
            let key = (row["a"].as_str().unwrap_or("").to_string(), row["b"].as_str().unwrap_or("").to_string());
            if pairs.get(&key).is_none_or(|had| had["count"].as_i64() < row["count"].as_i64()) {
                pairs.insert(key, row);
            }
        }
        let mut rows: Vec<Value> = pairs.into_values().collect();
        rows.sort_by_key(|r| r["last_at"].as_i64());
        if args.json {
            out.line(&Value::Array(rows).to_string());
        } else {
            if rows.is_empty() && silent.is_empty() {
                out.line("nobody has written to anybody there yet");
            }
            for r in rows {
                let waiting = match r["waiting"].as_i64().unwrap_or(0) {
                    0 => String::new(),
                    n => format!(", {n} not dealt with"),
                };
                out.line(&format!("{} and {}: {} message(s){waiting}", r["a"].as_str().unwrap_or(""), r["b"].as_str().unwrap_or(""), r["count"]));
            }
        }
        return Ok(report(out, &silent));
    }
    let (rows, silent) = gather(ctx, by, &scope, "messages", &flags, |_| message_rows(ctx, who.as_deref(), &args, after, before, count))?;
    // A message between two kernels is known to both: the word of the one
    // where it got furthest counts.
    let mut newest: BTreeMap<(i64, String), Value> = BTreeMap::new();
    for row in rows {
        let key = (row["at"].as_i64().unwrap_or(0), row["id"].as_str().unwrap_or("").to_string());
        if newest.get(&key).is_none_or(|had| rank(had["status"].as_str().unwrap_or("")) < rank(row["status"].as_str().unwrap_or(""))) {
            newest.insert(key, row);
        }
    }
    let rows: Vec<Value> = newest.into_values().collect();
    let rows = &rows[rows.len().saturating_sub(count.max(0) as usize)..];
    if args.json {
        out.line(&Value::Array(rows.to_vec()).to_string());
        return Ok(report(out, &silent));
    }
    if rows.is_empty() && silent.is_empty() {
        out.line(&format!("no messages{}", if args.waiting { " are waiting" } else { "" }));
    }
    for m in rows {
        let state = progress(m["status"].as_str().unwrap_or(""), m["attempts"].as_i64().unwrap_or(0));
        let result = match m["result"].as_str() {
            Some(result) if !result.trim().is_empty() => format!("\n⇒ {}", result.trim().replace('\n', "\n  ")),
            _ => String::new(),
        };
        out.line(&format!(
            "#{} {} {} → {}{}\n{}{result}\n",
            m["id"].as_str().unwrap_or(""),
            clock(m["at"].as_i64().unwrap_or(0)),
            m["from"].as_str().unwrap_or(""),
            m["to"].as_str().unwrap_or(""),
            if state.is_empty() { String::new() } else { format!("  ({state})") },
            m["body"].as_str().unwrap_or(""),
        ));
    }
    Ok(report(out, &silent))
}

// ------------------------------------------------------------------- show

fn show(ctx: &Ctx, by: &str, args: cli::Show, argv: Vec<String>, out: &mut dyn Out) -> Res<i32> {
    let (_, kernel) = index::address(&args.who, "process")?;
    let repo = &ctx.cfg.repo;
    // A process's branch is its address. If this repository has it, it is
    // read here, whatever became of the process and its kernel.
    if !repo.has_branch(&args.who) {
        if kernel != ctx.me {
            return relay(ctx, out, kernel, by, argv);
        }
        return Err(format!("there is no branch `{}` in {} (`jarvis ps @{}` lists the processes of this kernel)", args.who, repo.root.display(), ctx.me).into());
    }
    let text = match &args.path {
        None => repo.git(&["ls-tree", "-r", "--name-only", &args.who])?,
        Some(path) => repo
            .git(&["show", &format!("{}:{path}", args.who)])
            .map_err(|_| format!("{} has no file `{path}` (`jarvis show {}` lists its files)", args.who, args.who))?,
    };
    out.line(&text);
    Ok(0)
}

// -------------------------------------------------------------------- log

fn fact_row(r: &rusqlite::Row) -> rusqlite::Result<Value> {
    let data: String = r.get(4)?;
    Ok(json!({
        "id": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "proc": r.get::<_, String>(2)?,
        "kind": r.get::<_, String>(3)?, "data": serde_json::from_str::<Value>(&data).unwrap_or(Value::Null),
        "kernel": r.get::<_, String>(5)?, "n": r.get::<_, Option<i64>>(6)?,
    }))
}

const FACT_COLUMNS: &str = "seq, at, proc, kind, data, k, n";
/// Records are about a process when it did them, or when they bring it a message.
const ABOUT_PROC: &str = "(?2 IS NULL OR proc = ?2 OR (kind IN ('send', 'arrive') AND json_extract(data, '$.to') = ?2))";
/// A step is something an agent did inside a turn. There are many, so they
/// are shown only when asked for.
const STEPS: &str = "(?3 OR kind != 'step')";

/// The newest `count` of this kernel's records (about `who`, if given, and
/// between two places in its order, if given), oldest first.
fn fact_rows(ctx: &Ctx, who: Option<&str>, args: &cli::Log, count: i64) -> Res<Vec<Value>> {
    let mut stmt = ctx.conn.prepare(&format!("SELECT {FACT_COLUMNS} FROM records WHERE seq > ?1 AND {ABOUT_PROC} AND {STEPS} AND (?5 IS NULL OR seq < ?5) ORDER BY seq DESC LIMIT ?4"))?;
    let mut rows: Vec<Value> = stmt.query_map(params![args.after.unwrap_or(0), who, args.steps, count, args.before], fact_row)?.collect::<Result<_, _>>()?;
    rows.reverse();
    Ok(rows)
}

/// A record as a line for a terminal.
fn fact_text(fact: &Value) -> String {
    let d = &fact["data"];
    let text = |key: &str| d[key].as_str().unwrap_or("").to_string();
    let kind = fact["kind"].as_str().unwrap_or("");
    let what = match kind {
        "send" if d["request"].is_object() => format!("asks {}: jarvis {}", kernel_of(&text("to")), clip(&text("body"), 140)),
        "send" => format!("→ {}: {}", text("to"), clip(&text("body"), 140)),
        "arrive" if d["request"].is_object() => format!("is asked by {} (through {}): jarvis {}", text("from"), text("via"), clip(&text("body"), 120)),
        "arrive" => format!("← {} (through {}): {}", text("from"), text("via"), clip(&text("body"), 120)),
        "forward" => format!("the message {} was passed on to {}", text("message"), text("next")),
        "undeliverable" => format!("the message {} could not be delivered: {}", text("message"), text("why")),
        "receipt" => match (text("status").as_str(), d["result"].as_str()) {
            ("pending", _) => format!("its message {} arrived", text("message")),
            ("handled", Some(result)) if !result.is_empty() => format!("its request {} was carried out: {}", text("message"), clip(result, 120)),
            ("handled", _) => format!("its message {} was dealt with", text("message")),
            ("failed", result) => format!("its request {} could not be carried out: {}", text("message"), clip(result.unwrap_or(""), 120)),
            ("dropped", _) => format!("its message {} was dropped: its recipient was removed", text("message")),
            (_, result) => format!("its message {} could not be delivered: {}", text("message"), clip(result.unwrap_or(""), 120)),
        },
        "request_done" if d["code"] == 0 => format!("carried out the request {}: {}", text("message"), clip(&text("result"), 120)),
        "request_done" => format!("could not carry out the request {}: {}", text("message"), clip(&text("result"), 120)),
        "turn" => {
            let cost = d["cost_usd"].as_f64().map(|c| format!(" (${c:.3})")).unwrap_or_default();
            format!("turn{cost}: {}", clip(&text("reply"), 140))
        }
        "turn_start" => format!("turn started on {} message(s)", d["messages"].as_array().map_or(0, Vec::len)),
        "turn_failed" => format!("TURN FAILED ({} in a row, next try in {}s): {}", d["failures"], d["retry_in_s"], clip(&text("error"), 200)),
        "turn_interrupted" => "turn interrupted; its mail waits".to_string(),
        "turn_end" => "turn ended".to_string(),
        "spawn" if d["continued"] == true => format!("carried on here by {}", text("parent")),
        "spawn" => format!("created by {}", text("parent")),
        "stop" => format!("stopped by {}", text("by")),
        "start" => format!("started by {}", text("by")),
        "tag" => {
            let set = d["set"].as_object().map(|set| set.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
            format!("tagged by {}: {}", text("by"), set.unwrap_or_default())
        }
        "remove" => {
            let cost = d["cost_usd"].as_f64().map(|c| format!(", had cost ${c:.3}")).unwrap_or_default();
            format!("removed by {} (was at {}{cost})", text("by"), text("commit"))
        }
        "commit" => format!("commit {} {}", text("commit").chars().take(8).collect::<String>(), text("subject")),
        "commit_skipped" => format!("nothing committed: {}", text("why")),
        "commit_failed" => format!("COULD NOT COMMIT: {}", clip(&text("error"), 200)),
        "handled_by_code" => format!("its handler dealt with {} message(s)", d["messages"].as_array().map_or(0, Vec::len)),
        "run_start" => "service started (.jarvis/run)".to_string(),
        "run_exit" => format!("service exited (.jarvis/run): {}", text("status")),
        "kernel_error" => format!("PROBLEM: {}", clip(&text("error"), 300)),
        "kernel" => "a new kernel".to_string(),
        "up" => format!("is up (jarvis {})", text("version")),
        "closed" if text("kernel").is_empty() => "stopped".to_string(),
        "closed" => format!("{} is taken to be gone (said {})", text("kernel"), text("by")),
        "link_up" => format!("linked to {}", text("kernel")),
        "link_down" => format!("the link to {} went down: {}", text("kernel"), clip(&text("why"), 200)),
        "link_failing" => format!("the link {} cannot be made: {}", text("link"), clip(&text("why"), 300)),
        "link_refused" => format!("a link was refused: {}", clip(&text("why"), 200)),
        "step" => format!("  · {:<8} {}", text("step"), clip(&text("text"), 160)),
        other if *d == json!({}) => other.to_string(),
        other => format!("{other} {}", clip(&d.to_string(), 160)),
    };
    let proc = fact["proc"].as_str().unwrap_or("");
    // Who it is about; for what a kernel says of itself, that kernel.
    let about = if name_of(proc) == KERNEL { format!("[{}]", fact["kernel"].as_str().unwrap_or("")) } else { proc.to_string() };
    format!("{} {about:<24} {what}", clock(fact["at"].as_i64().unwrap_or(0)))
}

/// This kernel's records after `last` (steps left out), as lines for a
/// terminal. `last` is moved to the newest one. The kernel prints these as
/// it runs.
pub fn fact_lines(conn: &Connection, last: &mut i64) -> Res<Vec<String>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {FACT_COLUMNS} FROM records WHERE seq > ?1 AND kind != 'step' ORDER BY seq"))?;
    let facts: Vec<Value> = stmt.query_map([*last], fact_row)?.collect::<Result<_, _>>()?;
    Ok(facts
        .iter()
        .map(|fact| {
            *last = fact["id"].as_i64().unwrap_or(*last);
            fact_text(fact)
        })
        .collect())
}

fn log(ctx: &Ctx, by: &str, args: cli::Log, out: &mut dyn Out) -> Res<i32> {
    let (scope, who) = one_scope(ctx, &args.what, args.all)?;
    let count = args.count.unwrap_or(if args.json { 500 } else { 30 });
    let mut flags: Vec<String> = vec!["-n".into(), count.to_string()];
    if args.steps {
        flags.push("--steps".into());
    }
    for (flag, value) in [("--after", args.after), ("--before", args.before)] {
        if let Some(value) = value {
            flags.extend([flag.to_string(), value.to_string()]);
        }
    }
    let (mut rows, silent) = gather(ctx, by, &scope, "log", &flags, |_| fact_rows(ctx, who.as_deref(), &args, count))?;
    rows.sort_by_key(|r| (r["at"].as_i64(), r["kernel"].as_str().map(str::to_string), r["id"].as_i64()));
    let rows = &rows[rows.len().saturating_sub(count.max(0) as usize)..];
    if args.json {
        out.line(&Value::Array(rows.to_vec()).to_string());
        return Ok(report(out, &silent));
    }
    for fact in rows {
        if !out.line(&fact_text(fact)) {
            return Ok(0);
        }
    }
    if !args.follow {
        if rows.is_empty() && silent.is_empty() {
            out.line("nothing has happened there yet");
        }
        return Ok(report(out, &silent));
    }
    report(out, &silent);
    // From here on: every kernel's new records, as they are written. Each
    // carries on after the last one shown.
    let mut after: HashMap<String, i64> = HashMap::new();
    for fact in rows {
        if let (Some(kernel), Some(id)) = (fact["kernel"].as_str(), fact["id"].as_i64()) {
            let newest = after.entry(kernel.to_string()).or_insert(id);
            *newest = (*newest).max(id);
        }
    }
    follow(ctx, by, Followed { scope, all: args.all, after, steps: args.steps }, out, &|line: &Value, out: &mut dyn Out| match line["kind"].as_str() {
        Some("fact") => out.line(&fact_text(&line["fact"])),
        Some("lost") => {
            out.note(&format!("jarvis: the kernel `{}` is no longer followed: {}. It is followed again when it can be reached", line["kernel"].as_str().unwrap_or(""), line["why"].as_str().unwrap_or("")));
            true
        }
        _ => true,
    })
}

// ------------------------------------------------------------------ watch

fn watch(ctx: &Ctx, by: &str, args: cli::Watch, out: &mut dyn Out) -> Res<i32> {
    let mut after: HashMap<String, i64> = HashMap::new();
    for cursor in &args.after {
        match cursor.rsplit_once(':').and_then(|(kernel, n)| Some((kernel, n.parse::<i64>().ok()?))) {
            Some((kernel, n)) => after.insert(kernel.to_string(), n),
            None => return Err(format!("`{cursor}` is not a place to carry on from: write KERNEL:NUMBER, such as laptop:5120").into()),
        };
    }
    let scope = scope(ctx, &args.what, args.all)?;
    // Only this kernel: no going through other threads.
    if let [(kernel, only)] = scope.as_slice()
        && *kernel == ctx.me && !args.all {
            watch_here(ctx, only, after.get(&ctx.me).copied(), args.steps, out)?;
            return Ok(0);
        }
    follow(ctx, by, Followed { scope, all: args.all, after, steps: args.steps }, out, &|line: &Value, out: &mut dyn Out| out.line(&line.to_string()))
}

/// What to follow: which kernels and processes, whether also kernels that
/// come into reach later, where each kernel's facts carry on from, and
/// whether the steps inside turns are wanted.
struct Followed {
    scope: Scope,
    all: bool,
    after: HashMap<String, i64>,
    steps: bool,
}

/// What a kernel that is followed sends.
enum Fed {
    Line(String, Value),
    /// It ended, and why. It is followed again when it can be.
    Lost(String, String),
    /// Something changed here: new records, or which kernels can be reached.
    News,
}

/// Lines from a followed kernel, on their way to the command that follows it.
struct ToFeed {
    kernel: String,
    tx: Sender<Fed>,
}

impl Out for ToFeed {
    fn line(&mut self, text: &str) -> bool {
        let Ok(line) = serde_json::from_str::<Value>(text) else { return true };
        self.tx.send(Fed::Line(self.kernel.clone(), line)).is_ok()
    }
    fn note(&mut self, _text: &str) {}
}

/// Follow every kernel in the scope: each runs `jarvis watch` for its part
/// and its lines arrive here as they are printed. A kernel that is lost is
/// followed again, from where it stopped, when it can be reached; with
/// `all`, kernels that come into reach are followed too. Each line is given
/// to `show`, which says whether anyone is still reading.
fn follow(ctx: &Ctx, by: &str, what: Followed, out: &mut dyn Out, show: &dyn Fn(&Value, &mut dyn Out) -> bool) -> Res<i32> {
    let Followed { scope, all, mut after, steps } = what;
    let (tx, fed) = mpsc::channel::<Fed>();
    let news = ctx.follow();
    let wake = tx.clone();
    thread::spawn(move || {
        while news.recv().is_ok() {
            if wake.send(Fed::News).is_err() {
                break;
            }
        }
    });
    let mut parts: BTreeMap<String, Vec<String>> = scope.into_iter().collect();
    // Kernels whose lines are arriving now.
    let mut live: BTreeSet<String> = BTreeSet::new();
    // Kernels that were said to be lost, so that it is said once.
    let mut said_lost: BTreeSet<String> = BTreeSet::new();
    loop {
        if all {
            for kernel in ctx.known()? {
                parts.entry(kernel).or_default();
            }
        }
        let reachable = ctx.reachable();
        for (kernel, only) in &parts {
            if live.contains(kernel) || (*kernel != ctx.me && !reachable.contains(kernel)) {
                continue;
            }
            let mut argv: Vec<String> = std::iter::once("watch".to_string()).chain(typed(kernel, only)).collect();
            if let Some(n) = after.get(kernel) {
                argv.extend(["--after".to_string(), format!("{kernel}:{n}")]);
            }
            if steps {
                argv.push("--steps".into());
            }
            match feed(ctx, by, kernel, argv, only.clone(), after.get(kernel).copied(), steps, tx.clone()) {
                Ok(()) => {
                    live.insert(kernel.clone());
                    said_lost.remove(kernel);
                }
                Err(why) => {
                    let _ = tx.send(Fed::Lost(kernel.clone(), why.to_string()));
                }
            }
        }
        // Kernels that were named and cannot be reached are said to be lost, once.
        for kernel in parts.keys() {
            if !live.contains(kernel) && said_lost.insert(kernel.clone()) && !show(&json!({"kind": "lost", "kernel": kernel, "why": "it cannot be reached right now"}), out) {
                return Ok(0);
            }
        }
        match fed.recv() {
            Ok(Fed::Line(kernel, mut line)) => {
                line["kernel"] = json!(kernel);
                if let Some(id) = line["fact"]["id"].as_i64() {
                    after.insert(kernel, id);
                } else if let Some(cursor) = line["cursor"].as_i64() {
                    after.entry(kernel).or_insert(cursor);
                }
                if !show(&line, out) {
                    return Ok(0);
                }
            }
            Ok(Fed::Lost(kernel, why)) => {
                live.remove(&kernel);
                if said_lost.insert(kernel.clone()) && !show(&json!({"kind": "lost", "kernel": kernel, "why": why}), out) {
                    return Ok(0);
                }
            }
            Ok(Fed::News) => {}
            Err(_) => return Ok(0),
        }
    }
}

/// Start following one kernel: this one on a thread here, another one by
/// having it run the command and send its lines back.
#[allow(clippy::too_many_arguments)]
fn feed(ctx: &Ctx, by: &str, kernel: &str, argv: Vec<String>, only: Vec<String>, after: Option<i64>, steps: bool, tx: Sender<Fed>) -> Res<()> {
    let kernel = kernel.to_string();
    if kernel == ctx.me {
        let here = ctx.again()?;
        thread::spawn(move || {
            let mut out = ToFeed { kernel: kernel.clone(), tx: tx.clone() };
            let why = match watch_here(&here, &only, after, steps, &mut out) {
                Ok(()) => "it ended".to_string(),
                Err(e) => e.to_string(),
            };
            let _ = tx.send(Fed::Lost(kernel, why));
        });
        return Ok(());
    }
    let (to, from) = mpsc::channel::<Value>();
    let answer = ctx.mutate(Mutation::Subscribe { on: kernel.clone(), request: json!({"argv": argv, "by": by}), to })?;
    let sid = answer["o"][0].as_str().unwrap_or("").to_string();
    let to_kernel = ctx.tx.clone();
    thread::spawn(move || {
        // The kernel here drops its end when the other can no longer be reached.
        let mut why = "it cannot be reached any more".to_string();
        for item in from {
            if item.get("end").is_some() {
                why = match item["why"].as_str() {
                    Some(text) if !text.is_empty() => text.to_string(),
                    _ => "it stopped answering".to_string(),
                };
                break;
            }
            let Some(line) = item["line"].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok()) else { continue };
            if tx.send(Fed::Line(kernel.clone(), line)).is_err() {
                // Nobody follows any more: that kernel can stop sending.
                let (reply, _) = mpsc::channel();
                let _ = to_kernel.send(Event::Mutate(Mutation::Unsubscribe { sid }, reply));
                return;
            }
        }
        let _ = tx.send(Fed::Lost(kernel, why));
    });
    Ok(())
}

/// Print every change on this kernel as a JSON line, for as long as someone
/// is reading: all of it, or only what is about the processes in `only`.
fn watch_here(ctx: &Ctx, only: &[String], after: Option<i64>, steps: bool, out: &mut dyn Out) -> Res<()> {
    let conn = &ctx.conn;
    let news = ctx.follow();
    let mut cursor: i64 = match after {
        Some(after) => after,
        None => conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM records", [], |r| r.get(0))?,
    };
    let wanted = |addr: &str| only.is_empty() || only.iter().any(|one| one == addr);
    let line = |mut value: Value| -> String {
        value["kernel"] = json!(ctx.me);
        value.to_string()
    };
    // What was last said about each process.
    let mut said: HashMap<String, String> = HashMap::new();
    // The messages not dealt with yet, and their status when last seen.
    let mut open: HashMap<String, (String, i64)> = HashMap::new();
    let mut first = true;

    loop {
        if !out.alive() {
            return Ok(());
        }
        let mut touched: BTreeSet<String> = BTreeSet::new();

        // New facts, in order. (The first look only says how things stand.)
        let mut more = !first;
        while more {
            let mut stmt = conn.prepare_cached(&format!("SELECT {FACT_COLUMNS} FROM records WHERE seq > ?1 AND (?2 OR kind != 'step') ORDER BY seq LIMIT 2000"))?;
            let facts: Vec<Value> = stmt.query_map(params![cursor, steps], fact_row)?.collect::<Result<_, _>>()?;
            more = facts.len() == 2000;
            for fact in facts {
                cursor = fact["id"].as_i64().unwrap_or(cursor);
                let kind = fact["kind"].as_str().unwrap_or("").to_string();
                let proc = fact["proc"].as_str().unwrap_or("").to_string();
                let brings = kind == "send" || kind == "arrive";
                let to = if brings { fact["data"]["to"].as_str().unwrap_or("").to_string() } else { String::new() };
                if brings {
                    touched.insert(to.clone());
                }
                if kind != "step" {
                    touched.insert(proc.clone());
                }
                if !(wanted(&proc) || brings && wanted(&to)) {
                    continue;
                }
                let mut value = json!({"kind": "fact", "fact": fact});
                if brings {
                    value["message"] = value["fact"]["data"]["id"].as_str().and_then(|id| message_json(ctx, id)).unwrap_or(Value::Null);
                }
                if !out.line(&line(value)) {
                    return Ok(());
                }
            }
        }

        // Messages whose status changed.
        let mut stmt = conn.prepare_cached(&format!("SELECT id, status, attempts FROM messages WHERE {OPEN}"))?;
        let waiting: HashMap<String, (String, i64)> = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect::<Result<_, _>>()?;
        let mut changed: Vec<String> = waiting.iter().filter(|(id, state)| open.get(*id) != Some(*state)).map(|(id, _)| id.clone()).collect();
        changed.extend(open.keys().filter(|id| !waiting.contains_key(*id)).cloned());
        open = waiting;
        if !first {
            let mut changed: Vec<Value> = changed.iter().filter_map(|id| message_json(ctx, id)).collect();
            changed.sort_by_key(|m| m["at"].as_i64());
            for message in changed {
                let (from, to) = (message["from"].as_str().unwrap_or("").to_string(), message["to"].as_str().unwrap_or("").to_string());
                touched.insert(from.clone());
                touched.insert(to.clone());
                if !wanted(&from) && !wanted(&to) {
                    continue;
                }
                if !out.line(&line(json!({"kind": "message", "message": message}))) {
                    return Ok(());
                }
            }
        }

        // Processes: at first all of them, then those something happened to.
        let procs: Vec<Proc> = index::procs(conn)?.into_iter().filter(|p| wanted(&p.addr)).collect();
        for p in &procs {
            if !first && !touched.contains(&p.addr) && said.contains_key(&p.addr) {
                continue;
            }
            let described = describe(ctx, p)?;
            let text = described.to_string();
            if said.get(&p.addr) == Some(&text) {
                continue;
            }
            said.insert(p.addr.clone(), text);
            if !out.line(&line(json!({"kind": "process", "process": described}))) {
                return Ok(());
            }
        }
        let gone: Vec<String> = said.keys().filter(|addr| !procs.iter().any(|p| &p.addr == *addr)).cloned().collect();
        for addr in gone {
            said.remove(&addr);
            if !out.line(&line(json!({"kind": "gone", "id": addr}))) {
                return Ok(());
            }
        }
        if first {
            first = false;
            if !out.line(&line(json!({"kind": "ready", "cursor": cursor}))) {
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
