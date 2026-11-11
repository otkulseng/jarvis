//! The kernel: one program that hosts processes and passes messages on. It
//! holds no opinion about what any process is for.
//!
//! What it does:
//!
//! 1. keeps each of its processes' `.jarvis/run` going, if there is one;
//! 2. hands waiting messages to the process: first to its `.jarvis/handle`,
//!    if it has one, otherwise (or when the handler declines or fails) to its
//!    agent;
//! 3. commits whatever the process changed in its own directory;
//! 4. passes messages for other kernels on over its links, keeping each
//!    until the next kernel has confirmed it;
//! 5. writes down everything that happens as records (`index.rs`), and puts
//!    them in the store (`store.rs`).
//!
//! How it is built: one thread, the loop in `run`, owns everything the kernel
//! knows and is the only writer of its database. Everything else (a `jarvis`
//! command arriving on the socket, a turn ending, a line from a link, the
//! clock) reaches it as an `Event`, one at a time. Nothing is polled.
//!
//! Everything jarvis-specific about a process sits in the `.jarvis/`
//! directory of its tree. Its code is two optional executables there, both
//! run from the top of the tree:
//!
//!   .jarvis/run      a long-lived program, started while the process is
//!                    alive. If it exits with an error, its agent is told.
//!   .jarvis/handle   run once per message, with {"id","from","body",
//!                    "attempt"} on standard input. Exit 0: handled. Exit 3:
//!                    not mine, give it to the agent. Anything else: failed,
//!                    give it to the agent with the error output.
//!
//! The agent is started through a harness adapter, `harness/<name>` in the
//! repository: it gets the prompt on standard input, runs in the process's
//! directory, and prints JSON objects, one per line. A line with a `step`
//! key is something the agent did, and is noted at once. The last line,
//! {"session", "reply", ...}, is how the turn ended.
//!
//! What the kernel promises, whatever goes wrong:
//!
//! - A problem with one process does not stop the others.
//! - A message is never dropped. It waits until a turn on it has finished,
//!   and the kernel keeps offering it until one does. A message for another
//!   kernel is kept here until the next kernel on the way has it.
//! - At most one turn is at work in a process's directory, also after the
//!   kernel itself was killed and started again.
//! - When a message is given a second time, the recipient is told so.
//! - Ending a program ends everything it started.

use crate::children;
use crate::cli;
use crate::git::{self, Repo};
use crate::index::{self, HUMAN, KERNEL, Proc, Rec, Res, at, kernel_of, name_of};
use crate::net;
use crate::serve;
use crate::store::Store;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long a program gets to end by itself after being asked to.
const GRACE: Duration = Duration::from_secs(3);
/// A message that went through this many kernels is going in circles.
const MOST_HOPS: i64 = 32;
/// A kernel further away than this many links counts as out of reach.
const FARTHEST: u64 = 16;

/// Where a kernel is and what it uses. Fixed while it runs.
pub struct Config {
    pub name: String,
    pub identity: String,
    pub repo: Repo,
    pub store_dir: PathBuf,
    /// The kernel's own directory: its database, its processes' directories.
    pub state: PathBuf,
    /// Where the kernels of this machine put their sockets.
    pub run_dir: PathBuf,
    pub ship_every: Duration,
    pub scan_every: Duration,
}

impl Config {
    pub fn db_path(&self) -> PathBuf {
        self.state.join("kernel.db")
    }

    /// The working directory of a process: a checkout of its branch.
    pub fn proc_dir(&self, name: &str) -> PathBuf {
        self.state.join("proc").join(name)
    }

    pub fn guide(&self) -> PathBuf {
        self.state.join("GUIDE.md")
    }

    fn run_log(&self, name: &str) -> PathBuf {
        self.state.join("log").join(format!("{name}.run.log"))
    }

    fn harness(&self, name: &str) -> PathBuf {
        self.repo.root.join("harness").join(name)
    }
}

/// Where the kernels of this machine put their sockets: `$JARVIS_RUN`, or a
/// directory of the user's under /tmp. It is on the machine's own disk
/// whatever else is on network storage.
pub fn run_dir() -> PathBuf {
    match std::env::var("JARVIS_RUN") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(format!("/tmp/jarvis-{}", unsafe { libc::getuid() })),
    }
}

/// What the kernel shares with the threads that answer commands.
pub struct Shared {
    /// Whoever follows the records (`jarvis watch`, `jarvis log -f`) is told
    /// here that there are new ones.
    pub followers: Mutex<Vec<SyncSender<()>>>,
    pub net: Mutex<NetView>,
    pub next_link: AtomicU64,
}

/// The kernels and links as they are now, for commands that show them.
#[derive(Clone, Default)]
pub struct NetView {
    pub reachable: Vec<Reachable>,
    pub links: Vec<LinkView>,
}

#[derive(Clone)]
pub struct Reachable {
    pub name: String,
    pub hops: u64,
    pub via: String,
    pub procs: Vec<String>,
}

#[derive(Clone)]
pub struct LinkView {
    pub spec: Option<String>,
    pub peer: Option<String>,
    pub problem: Option<String>,
}

/// Everything that can happen to a kernel.
pub enum Event {
    /// A command wants something changed. The answer goes back on the sender.
    Mutate(Mutation, Sender<Result<Value, String>>),
    TurnDone(Done),
    Step { name: String, value: Value },
    RunExit { name: String, pid: u32, status: String, ok: bool },
    LinkOpen { id: u64, spec: Option<String>, tx: Sender<String>, close: net::Close },
    LinkLine { id: u64, value: Value },
    LinkDown { id: u64, why: String },
    /// A command another kernel asked this one to carry out has an answer.
    Answered { rid: String, to: String, body: Value },
    Tick,
}

/// A change a command asks for. The answer is `{"o": [lines], "e": [notes]}`.
pub enum Mutation {
    Spawn { args: cli::Spawn, by: String, cwd: String, prompt: Option<String> },
    SetStatus { who: Vec<String>, alive: bool, by: String },
    Remove { who: String, keep_branch: bool, by: String },
    Tag { who: String, tags: Vec<(String, String)>, by: String },
    Send { from: String, to: String, body: String },
    Link { spec: String, remove: bool },
    CloseKernel { name: String, by: String },
    Ship,
    /// Have another kernel carry out a command.
    Remote { on: String, request: Value },
}

fn said(lines: Vec<String>, notes: Vec<String>) -> Value {
    json!({"o": lines, "e": notes})
}

/// Set when the kernel is asked to stop (Ctrl-C, `kill`, a closed terminal).
static STOP: AtomicBool = AtomicBool::new(false);

/// How long to wait before offering mail again after a turn failed: 5 seconds
/// after the first failure, doubling with each further one, at most 5
/// minutes. It never gives up; `jarvis stop` does that.
fn retry_delay(failures: i64) -> Duration {
    let doublings = (failures - 1).clamp(0, 6) as u32;
    Duration::from_secs((5 * 2u64.pow(doublings)).min(300))
}

/// What the kernel remembers about a process while it is up.
#[derive(Default)]
struct Live {
    /// The pid of its `.jarvis/run`, while it runs.
    run: Option<u32>,
    /// The kernel itself is ending the service (to start it afresh, or
    /// because the process was stopped): its exit is then no news.
    run_ending: bool,
    /// Set when `.jarvis/run` has exited: the commit it was at. It is started
    /// again only once the process's code is at a different commit.
    run_down_at: Option<String>,
    /// A turn is in progress. `cancel` tells that turn to stop, `turn` is the
    /// number of the record that began it, and `turn_began_at` is the commit
    /// the directory was at then.
    busy: bool,
    cancel: Option<Arc<AtomicBool>>,
    turn: Option<i64>,
    turn_began_at: Option<String>,
    /// Turns that failed in a row, and when mail is offered next.
    failures: i64,
    retry_at: Option<Instant>,
    /// The last problem the kernel had with this process, so that a problem
    /// that persists is written down once.
    trouble: Option<String>,
    /// It is being removed; this happens once nothing of it runs any more.
    removing: Option<Removal>,
}

struct Removal {
    keep_branch: bool,
    by: String,
    reply: Sender<Result<Value, String>>,
}

struct Mail {
    id: String,
    from: String,
    body: String,
    /// 1 the first time it is given to the process, more after that.
    attempt: i64,
}

/// Everything a turn needs, so it can run on its own thread.
struct Job {
    name: String,
    addr: String,
    kernel: String,
    dir: PathBuf,
    db_path: PathBuf,
    harness: PathBuf,
    session: Option<String>,
    env: Vec<(String, String)>,
    mail: Vec<Mail>,
    /// Something about the directory its agent should know (see `git::not_committable`).
    note: Option<String>,
    cancel: Arc<AtomicBool>,
    tx: Sender<Event>,
}

enum Agent {
    /// The process's code handled every message.
    NotNeeded,
    Replied(Value),
    Failed(String),
    /// The kernel ended the turn (the process was stopped).
    Interrupted,
}

pub struct Done {
    name: String,
    by_code: Vec<String>,
    for_agent: Vec<String>,
    agent: Agent,
}

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Copy a directory and everything in it, links kept as links.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&source)?, &target)?;
        } else {
            fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ turns

/// Run one program of a turn to its end, tracked so that it can be ended
/// from outside, and return what it printed. Each line it prints is shown to
/// `on_line` as it arrives; the lines `on_line` takes (by returning true) are
/// left out of what is returned.
fn run_tracked(
    job: &Job,
    conn: Option<&Connection>,
    kind: &str,
    mut command: Command,
    input: &str,
    on_line: &mut dyn FnMut(&str) -> bool,
) -> Result<Output, String> {
    if job.cancel.load(Ordering::SeqCst) {
        return Err("interrupted".into());
    }
    let mut child = command
        .current_dir(&job.dir)
        .envs(job.env.iter().cloned())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not start it: {e}"))?;
    let pid = child.id();
    if let Some(conn) = conn {
        children::track(conn, pid, &job.name, kind);
    }
    // The turn may have been cancelled while this program was being started.
    if job.cancel.load(Ordering::SeqCst) {
        children::signal(pid, libc::SIGKILL);
    }
    // A program that does not read its input is not an error.
    let _ = child.stdin.take().expect("stdin is piped").write_all(input.as_bytes());
    // Its errors are collected on the side, so that it can never be stuck
    // writing them while its output is being read here.
    let mut errors = child.stderr.take().expect("stderr is piped");
    let errors = thread::spawn(move || {
        let mut all = Vec::new();
        let _ = errors.read_to_end(&mut all);
        all
    });
    let mut stdout = Vec::new();
    let mut reader = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if !on_line(String::from_utf8_lossy(&line).trim_end()) {
                    stdout.extend_from_slice(&line);
                }
            }
        }
    }
    let out = child
        .wait()
        .map(|status| Output { status, stdout, stderr: errors.join().unwrap_or_default() })
        .map_err(|e| e.to_string());
    // Nothing it started outlives it.
    children::signal(pid, libc::SIGKILL);
    if let Some(conn) = conn {
        children::untrack(conn, pid);
    }
    out
}

enum Handled {
    Yes,
    NotMine,
    Failed(String),
}

fn run_handle(job: &Job, conn: Option<&Connection>, mail: &Mail) -> Handled {
    let input = json!({"id": mail.id, "from": mail.from, "body": mail.body, "attempt": mail.attempt}).to_string();
    match run_tracked(job, conn, "handle", Command::new(job.dir.join(".jarvis/handle")), &input, &mut |_| false) {
        Ok(out) if out.status.success() => Handled::Yes,
        Ok(out) if out.status.code() == Some(3) => Handled::NotMine,
        Ok(out) => Handled::Failed(format!("{}\n{}", out.status, tail(&String::from_utf8_lossy(&out.stderr), 30))),
        Err(e) => Handled::Failed(e),
    }
}

fn run_agent(job: &Job, conn: Option<&Connection>, prompt: &str) -> Result<Value, String> {
    if !is_executable(&job.harness) {
        return Err(format!("no harness adapter at {}", job.harness.display()));
    }
    let mut command = Command::new(&job.harness);
    command.env("JARVIS_SESSION", job.session.as_deref().unwrap_or(""));
    // What the agent does is noted while it does it.
    let mut step = |line: &str| match serde_json::from_str::<Value>(line) {
        Ok(value) if value.get("step").is_some() => {
            let _ = job.tx.send(Event::Step { name: job.name.clone(), value });
            true
        }
        _ => false,
    };
    let out = run_tracked(job, conn, "agent", command, prompt, &mut step)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(format!("harness exited with {}:\n{}\n{}", out.status, tail(&String::from_utf8_lossy(&out.stderr), 30), tail(&stdout, 10)));
    }
    let value: Value = serde_json::from_str(stdout.trim())
        .map_err(|_| format!("harness did not print a JSON object:\n{}", tail(&stdout, 10)))?;
    if !value["reply"].is_string() {
        return Err(format!("harness output has no `reply`:\n{}", tail(&stdout, 10)));
    }
    Ok(value)
}

/// One turn of a process: its waiting mail goes to its code, then its agent.
fn turn(job: Job) -> Done {
    let conn = index::open(&job.db_path).ok();
    let conn = conn.as_ref();
    let cancelled = || job.cancel.load(Ordering::SeqCst);
    let has_handle = is_executable(&job.dir.join(".jarvis/handle"));
    let (mut by_code, mut for_agent, mut prompt) = (Vec::new(), Vec::new(), String::new());
    for mail in &job.mail {
        let mut failure = None;
        if has_handle {
            match run_handle(&job, conn, mail) {
                Handled::Yes => {
                    by_code.push(mail.id.clone());
                    continue;
                }
                Handled::NotMine => {}
                Handled::Failed(error) => failure = Some(error),
            }
        }
        if cancelled() {
            break;
        }
        for_agent.push(mail.id.clone());
        prompt.push_str(&format!("--- message {} from {} ---\n{}\n", mail.id, mail.from, mail.body));
        if mail.attempt > 1 {
            prompt.push_str(&format!(
                "[jarvis] This message was given to you before (this is attempt {}), and that turn did not finish. \
                 You may already have acted on it: check what you did before repeating anything.\n",
                mail.attempt
            ));
        }
        if let Some(error) = failure {
            prompt.push_str(&format!("[jarvis] Your .jarvis/handle failed on this message:\n{error}\n"));
        }
        prompt.push('\n');
    }
    let agent = if cancelled() {
        Agent::Interrupted
    } else if for_agent.is_empty() {
        Agent::NotNeeded
    } else {
        let mut header = format!(
            "[jarvis] You are the process `{}` on the kernel `{}`; your address is `{}`. {} new message(s). \
             To answer one, send to the address it is from.\n",
            job.name,
            job.kernel,
            job.addr,
            for_agent.len()
        );
        if let Some(note) = &job.note {
            header.push_str(&format!(
                "[jarvis] In your directory, {note}. Nothing there is committed for you until that is put right.\n"
            ));
        }
        match run_agent(&job, conn, &format!("{header}\n{prompt}")) {
            Ok(out) => Agent::Replied(out),
            Err(_) if cancelled() => Agent::Interrupted,
            Err(error) => Agent::Failed(error),
        }
    };
    Done { name: job.name, by_code, for_agent, agent }
}

// ----------------------------------------------------------------- kernel

/// A link to another kernel, while it is up.
struct Link {
    tx: Sender<String>,
    close: net::Close,
    /// The kernel at the other end, once it has said who it is.
    peer: Option<String>,
    heard: Instant,
}

/// A link this kernel was told to keep (`jarvis link`), up or not.
struct Dial {
    link: Option<u64>,
    next_try: Instant,
    wait: Duration,
    problem: Option<String>,
}

/// What is known of a kernel that can be reached over links.
struct Known {
    identity: String,
    hops: u64,
    link: u64,
    procs: Vec<String>,
}

pub struct Kernel {
    cfg: Arc<Config>,
    me: String,
    db: Connection,
    store: Store,
    shared: Arc<Shared>,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    /// The number the next record gets.
    next_n: i64,
    /// Records were written since the followers were last told.
    dirty: bool,
    /// The newest record already printed on standard error.
    printed: i64,
    live: HashMap<String, Live>,
    links: HashMap<u64, Link>,
    dials: HashMap<String, Dial>,
    /// What each link's neighbour said it can reach.
    offers: HashMap<u64, HashMap<String, Known>>,
    /// The kernels that can be reached, and through which link: for each,
    /// the nearest of what the neighbours offer.
    table: HashMap<String, Known>,
    /// Messages handed to a link and not confirmed by it yet.
    awaiting: HashMap<String, u64>,
    /// Messages not passed on until the routes change (they went in circles).
    paused: HashSet<String>,
    /// Commands given to other kernels that have not been answered yet.
    asked: HashMap<String, (Sender<Result<Value, String>>, Instant)>,
    asked_n: u64,
    last_ship: Instant,
    last_scan: Instant,
    last_ping: Instant,
}

/// `jarvis up`: set a kernel up, run it until it is told to stop.
pub fn up(args: cli::Up, store: Option<String>) -> Res<()> {
    let name = args.name.clone();
    index::check_name(&name, "kernel")?;
    if matches!(name.as_str(), HUMAN | KERNEL) {
        return Err(format!("`{name}` is reserved; choose another name for the kernel").into());
    }
    let here = std::env::current_dir()?;
    let absolute = |text: &str| -> PathBuf { here.join(text) };
    let repo = Repo::find(&args.repo.as_deref().map(absolute).unwrap_or(here.clone()))?;
    let state = args.state.as_deref().map(absolute).unwrap_or_else(|| repo.root.join(".jarvis/kernel").join(&name));
    let store_dir = store
        .or_else(|| std::env::var("JARVIS_STORE").ok().filter(|s| !s.is_empty()))
        .as_deref()
        .map(absolute)
        .unwrap_or_else(|| repo.root.join(".jarvis/store"));
    for dir in [state.join("proc"), state.join("log")] {
        fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    // What the kernel keeps inside the repository's directory is not code.
    hide_from_git(&repo, &[&state, &store_dir]);

    let run_dir = run_dir();
    fs::create_dir_all(&run_dir)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&run_dir, fs::Permissions::from_mode(0o700));
    }

    let db = index::open(&state.join("kernel.db"))?;
    match index::get_meta(&db, "name")? {
        Some(was) if was != name => {
            return Err(format!("{} is the directory of the kernel `{was}`, not of `{name}`", state.display()).into());
        }
        _ => index::set_meta(&db, "name", &name)?,
    }
    let (identity, new) = match index::get_meta(&db, "identity")? {
        Some(identity) => (identity, false),
        None => (index::random_hex(8)?, true),
    };
    // The name is taken in the repository and in the store before anything
    // is written under it. Both refuse a second kernel of the same name.
    repo.claim(&name, &identity)?;
    let store = Store::open(&store_dir, &db)?;
    store.announce(&name, &identity, index::now_ms())?;
    index::set_meta(&db, "identity", &identity)?;

    let cfg = Arc::new(Config {
        name: name.clone(),
        identity,
        repo,
        store_dir,
        state,
        run_dir,
        ship_every: Duration::from_secs_f64(args.ship_every.max(0.05)),
        scan_every: Duration::from_secs_f64(args.scan_every.max(0.05)),
    });
    fs::write(cfg.guide(), include_str!("../GUIDE.md"))?;

    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared { followers: Mutex::new(Vec::new()), net: Mutex::new(NetView::default()), next_link: AtomicU64::new(1) });
    // Being the one kernel of this name on this machine is having its socket.
    serve::listen(cfg.clone(), tx.clone(), shared.clone())?;

    let next_n: i64 = db.query_row("SELECT COALESCE(MAX(n), 0) + 1 FROM records WHERE k = ?1", [&name], |r| r.get(0))?;
    let printed: i64 = db.query_row("SELECT COALESCE(MAX(seq), 0) FROM records", [], |r| r.get(0))?;
    let long_ago = Instant::now() - Duration::from_secs(3600);
    let mut kernel = Kernel {
        cfg,
        me: name,
        db,
        store,
        shared,
        tx,
        rx,
        next_n,
        dirty: false,
        printed,
        live: HashMap::new(),
        links: HashMap::new(),
        dials: HashMap::new(),
        offers: HashMap::new(),
        table: HashMap::new(),
        awaiting: HashMap::new(),
        paused: HashSet::new(),
        asked: HashMap::new(),
        asked_n: 0,
        last_ship: Instant::now(),
        last_scan: long_ago,
        last_ping: Instant::now(),
    };
    if new {
        let identity = kernel.cfg.identity.clone();
        kernel.rec(&at(KERNEL, &kernel.me.clone()), "kernel", json!({"identity": identity}))?;
    }
    kernel.run()
}

/// List directories the kernel made inside the repository's own directory
/// in the repository's private list of things that are not code.
fn hide_from_git(repo: &Repo, dirs: &[&PathBuf]) {
    let exclude = repo.git_dir.join("info/exclude");
    let mut listed = fs::read_to_string(&exclude).unwrap_or_default();
    let mut changed = false;
    for dir in dirs {
        let Ok(inside) = dir.strip_prefix(&repo.root) else { continue };
        // Its top directory under `.jarvis/`, such as `/.jarvis/kernel/`.
        let top: PathBuf = inside.components().take(2).collect();
        let line = format!("/{}/", top.display());
        if !listed.lines().any(|l| l == line) {
            if !listed.is_empty() && !listed.ends_with('\n') {
                listed.push('\n');
            }
            listed.push_str(&line);
            listed.push('\n');
            changed = true;
        }
    }
    if changed {
        let _ = fs::create_dir_all(repo.git_dir.join("info"));
        let _ = fs::write(exclude, listed);
    }
}

impl Kernel {
    fn addr(&self, name: &str) -> String {
        at(name, &self.me)
    }

    /// How to call an address in what people and agents read: without the
    /// kernel when it is this one.
    fn label(&self, addr: &str) -> String {
        if kernel_of(addr) == self.me { name_of(addr).to_string() } else { addr.to_string() }
    }

    /// Write a record down. Returns its number.
    fn rec(&mut self, proc: &str, kind: &str, data: Value) -> Res<i64> {
        let n = self.next_n;
        index::insert(&self.db, &Rec { k: self.me.clone(), n: Some(n), at: index::now_ms(), kind: kind.into(), proc: proc.into(), data })?;
        self.next_n += 1;
        self.dirty = true;
        Ok(n)
    }

    /// Write a record down where failing to must not stop what is going on.
    fn note(&mut self, proc: &str, kind: &str, data: Value) {
        if let Err(e) = self.rec(proc, kind, data) {
            eprintln!("jarvis: could not write down `{kind}` for {proc}: {e}");
        }
    }

    fn find(&self, name: &str) -> Res<Option<Proc>> {
        index::find_proc(&self.db, &self.addr(name))
    }

    fn run(&mut self) -> Res<()> {
        self.take_over()?;
        ctrlc::set_handler(|| STOP.store(true, Ordering::SeqCst))?;
        let ticker = self.tx.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(250));
                if ticker.send(Event::Tick).is_err() {
                    break;
                }
            }
        });
        eprintln!(
            "jarvis: kernel `{}` is up. Code in {}, records in {}, its own state in {}. Ctrl-C stops it.",
            self.me,
            self.cfg.repo.root.display(),
            self.cfg.store_dir.display(),
            self.cfg.state.display()
        );
        let me = self.addr(KERNEL);
        self.rec(&me, "up", json!({"pid": std::process::id()}))?;
        let links: Vec<String> = {
            let mut stmt = self.db.prepare("SELECT spec FROM links")?;
            stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for spec in links {
            self.dials.insert(spec, Dial { link: None, next_try: Instant::now(), wait: Duration::from_secs(2), problem: None });
        }
        for proc in index::procs(&self.db, true)? {
            if proc.kernel == self.me {
                self.tend(&proc.name);
            }
        }
        self.publish_net();

        while !STOP.load(Ordering::SeqCst) {
            let Ok(event) = self.rx.recv() else { break };
            self.handle(event);
            // Whatever else is waiting is dealt with before anyone is told.
            while let Ok(event) = self.rx.try_recv() {
                self.handle(event);
            }
            if self.dirty {
                self.dirty = false;
                self.print_new();
                self.shared.followers.lock().unwrap().retain(|f| !matches!(f.try_send(()), Err(mpsc::TrySendError::Disconnected(_))));
            }
        }
        self.shut_down();
        Ok(())
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Mutate(mutation, reply) => self.mutate(mutation, reply),
            Event::TurnDone(done) => self.finish(done),
            Event::Step { name, value } => {
                let rec = Rec { k: self.me.clone(), n: None, at: index::now_ms(), kind: "step".into(), proc: self.addr(&name), data: value };
                if index::insert(&self.db, &rec).is_ok() {
                    self.dirty = true;
                }
            }
            Event::RunExit { name, pid, status, ok } => self.run_exit(&name, pid, &status, ok),
            Event::LinkOpen { id, spec, tx, close } => self.link_open(id, spec, tx, close),
            Event::LinkLine { id, value } => self.link_line(id, value),
            Event::LinkDown { id, why } => self.link_down(id, &why),
            Event::Answered { rid, to, body } => {
                self.route_to(&to, json!({"t": "res", "rid": rid, "dst": to, "body": body}));
            }
            Event::Tick => self.tick(),
        }
    }

    /// End what a kernel that was killed left running. (That this is the only
    /// kernel of its name here was settled when it took the socket.)
    fn take_over(&mut self) -> Res<()> {
        let left = children::tracked(&self.db, None);
        if !left.is_empty() {
            eprintln!("jarvis: ending {} program(s) this kernel left running when it last stopped", left.len());
            children::end_all(left);
        }
        self.db.execute("DELETE FROM children", [])?;
        Ok(())
    }

    /// Stop: end every program the kernel started, say so, and put the last
    /// records in the store. Mail that was being worked on is given again
    /// next time.
    fn shut_down(&mut self) {
        eprintln!("jarvis: stopping");
        for live in self.live.values() {
            if let Some(cancel) = &live.cancel {
                cancel.store(true, Ordering::SeqCst);
            }
        }
        children::end_all(children::tracked(&self.db, None));
        let _ = self.db.execute("DELETE FROM children", []);
        for link in self.links.values() {
            (link.close)();
        }
        let me = self.addr(KERNEL);
        self.note(&me, "closed", json!({"by": me}));
        if let Err(e) = self.store.ship(&self.db, &self.me) {
            eprintln!("jarvis: could not put the last records in the store: {e}");
        }
        let _ = fs::remove_file(self.cfg.run_dir.join(format!("{}.sock", self.me)));
    }

    /// The clock: what waits for a time and not for an event.
    fn tick(&mut self) {
        let now = Instant::now();
        // Mail whose turn failed is offered again after its pause.
        let due: Vec<String> = self.live.iter().filter(|(_, l)| l.retry_at.is_some_and(|at| now >= at)).map(|(name, _)| name.clone()).collect();
        for name in due {
            if let Some(live) = self.live.get_mut(&name) {
                live.retry_at = None;
            }
            self.tend(&name);
        }
        if now.duration_since(self.last_ship) >= self.cfg.ship_every {
            self.last_ship = now;
            if let Err(e) = self.store.ship(&self.db, &self.me) {
                eprintln!("jarvis: could not write to the store: {e}");
            }
        }
        if now.duration_since(self.last_scan) >= self.cfg.scan_every {
            self.last_scan = now;
            match self.store.ingest(&self.db, Some(&self.me)) {
                Ok(0) => {}
                Ok(_) => self.dirty = true,
                Err(e) => eprintln!("jarvis: could not read the store: {e}"),
            }
        }
        // Links that should be up and are not.
        let wanted: Vec<String> = self.dials.iter().filter(|(_, d)| d.link.is_none() && now >= d.next_try).map(|(spec, _)| spec.clone()).collect();
        for spec in wanted {
            let id = self.shared.next_link.fetch_add(1, Ordering::SeqCst);
            if let Some(dial) = self.dials.get_mut(&spec) {
                dial.link = Some(id);
            }
            net::dial(spec, id, self.cfg.clone(), self.tx.clone());
        }
        // A link that has gone silent is taken down, to be made again.
        if now.duration_since(self.last_ping) >= Duration::from_secs(20) {
            self.last_ping = now;
            let silent: Vec<u64> = self.links.iter().filter(|(_, l)| now.duration_since(l.heard) > Duration::from_secs(70)).map(|(id, _)| *id).collect();
            for id in silent {
                if let Some(link) = self.links.get(&id) {
                    (link.close)();
                }
            }
            for link in self.links.values() {
                let _ = link.tx.send(json!({"t": "ping"}).to_string());
            }
        }
        // A command another kernel never answered.
        let stale: Vec<String> = self.asked.iter().filter(|(_, (_, since))| now.duration_since(*since) > Duration::from_secs(30)).map(|(rid, _)| rid.clone()).collect();
        for rid in stale {
            if let Some((reply, _)) = self.asked.remove(&rid) {
                let _ = reply.send(Err("the other kernel did not answer within 30 seconds; what it did, if anything, its records will show".into()));
            }
        }
    }

    /// Print what was written down since last time, as `jarvis log` would.
    fn print_new(&mut self) {
        match serve::fact_lines(&self.db, &self.me, self.printed, None, None, false, &mut self.printed) {
            Ok(lines) => {
                for line in lines {
                    eprintln!("{line}");
                }
            }
            Err(e) => eprintln!("jarvis: could not read the records: {e}"),
        }
    }

    /// Note a problem the kernel had with one process, once.
    fn trouble(&mut self, name: &str, error: &str) {
        let live = self.live.entry(name.to_string()).or_default();
        if live.trouble.as_deref() == Some(error) {
            return;
        }
        live.trouble = Some(error.to_string());
        eprintln!("jarvis: problem with {name}: {error}");
        let addr = self.addr(name);
        self.note(&addr, "kernel_error", json!({"error": error}));
    }

    // ------------------------------------------------------- processes

    /// Environment every program of a process gets: who it is, which kernel
    /// carries out its `jarvis` commands, and its name on the commits it makes.
    fn proc_env(&self, name: &str) -> Vec<(String, String)> {
        let mut path = std::env::var("PATH").unwrap_or_default();
        if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
            path = format!("{}:{path}", dir.display());
        }
        vec![
            ("JARVIS_KERNEL".into(), self.me.clone()),
            ("JARVIS_RUN".into(), self.cfg.run_dir.display().to_string()),
            ("JARVIS_ID".into(), self.addr(name)),
            ("JARVIS_NAME".into(), name.to_string()),
            ("JARVIS_GUIDE".into(), self.cfg.guide().display().to_string()),
            ("JARVIS_STORE".into(), self.cfg.store_dir.display().to_string()),
            ("GIT_AUTHOR_NAME".into(), name.to_string()),
            ("GIT_COMMITTER_NAME".into(), name.to_string()),
            ("PATH".into(), path),
        ]
    }

    /// Look after one process: its service, and its mail.
    fn tend(&mut self, name: &str) {
        let Ok(Some(proc)) = self.find(name) else { return };
        if self.live.get(name).is_some_and(|l| l.removing.is_some()) {
            return;
        }
        if proc.status != "alive" {
            self.put_to_rest(name);
            return;
        }
        self.live.entry(name.to_string()).or_default();
        let dir = self.cfg.proc_dir(name);
        // A service that cannot be started must not keep mail from the agent
        // who could fix it, so both are attempted.
        let service = self.tend_run(&proc, &dir);
        let mail = self.dispatch(&proc, &dir);
        match service.and(mail) {
            Ok(()) => {
                if let Some(live) = self.live.get_mut(name) {
                    live.trouble = None;
                }
            }
            Err(e) => self.trouble(name, &e.to_string()),
        }
    }

    /// End a process's service and any turn in progress.
    fn put_to_rest(&mut self, name: &str) {
        self.end_run(name);
        let Some(live) = self.live.get_mut(name) else { return };
        if let Some(cancel) = &live.cancel {
            if !cancel.swap(true, Ordering::SeqCst) {
                // Ended in the background, so the others are not kept waiting.
                let programs: Vec<(u32, String)> = {
                    let Ok(mut stmt) = self.db.prepare("SELECT pid, started FROM children WHERE proc = ?1 AND kind != 'run'") else { return };
                    stmt.query_map([name], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
                };
                thread::spawn(move || children::end_all(programs));
            }
        }
        // Being started again is a fresh start.
        live.failures = 0;
        live.retry_at = None;
    }

    /// Ask a process's service to end. Its exit arrives as an event.
    fn end_run(&mut self, name: &str) {
        let Some(live) = self.live.get_mut(name) else { return };
        let Some(pid) = live.run else { return };
        if live.run_ending {
            return;
        }
        live.run_ending = true;
        thread::spawn(move || {
            children::signal(pid, libc::SIGTERM);
            let deadline = Instant::now() + GRACE;
            // `kill` with signal 0 only asks whether the program is still there.
            while Instant::now() < deadline && unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
                thread::sleep(Duration::from_millis(50));
            }
            children::signal(pid, libc::SIGKILL);
        });
    }

    /// Start `.jarvis/run` when it should be up and is not.
    fn tend_run(&mut self, proc: &Proc, dir: &Path) -> Res<()> {
        let name = proc.name.as_str();
        let live = self.live.get(name).expect("entry exists");
        let program = dir.join(".jarvis/run");
        if live.run.is_some() || live.busy || !is_executable(&program) {
            return Ok(());
        }
        if let Some(commit) = &live.run_down_at {
            if git::head(dir).as_deref() == Some(commit.as_str()) {
                return Ok(());
            }
        }
        let log = File::create(self.cfg.run_log(name))?;
        let child = Command::new(&program)
            .current_dir(dir)
            .envs(self.proc_env(name))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn();
        match child {
            Ok(mut child) => {
                let pid = child.id();
                children::track(&self.db, pid, name, "run");
                let live = self.live.get_mut(name).expect("entry exists");
                live.run = Some(pid);
                live.run_ending = false;
                live.run_down_at = None;
                // Its end is an event like any other.
                let (tx, name) = (self.tx.clone(), name.to_string());
                thread::spawn(move || {
                    let (status, ok) = match child.wait() {
                        Ok(status) => (status.to_string(), status.success()),
                        Err(e) => (e.to_string(), false),
                    };
                    let _ = tx.send(Event::RunExit { name, pid, status, ok });
                });
                self.rec(&proc.addr, "run_start", json!({}))?;
            }
            Err(e) => {
                self.live.get_mut(name).expect("entry exists").run_down_at = Some(git::head(dir).unwrap_or_default());
                self.rec(&proc.addr, "run_exit", json!({"status": "could not start", "output": e.to_string()}))?;
                let me = self.addr(KERNEL);
                self.post(&me, &proc.addr, &format!("[jarvis] Your .jarvis/run could not be started: {e}"))?;
            }
        }
        Ok(())
    }

    /// A process's service has ended.
    fn run_exit(&mut self, name: &str, pid: u32, status: &str, ok: bool) {
        let Some(live) = self.live.get_mut(name) else { return };
        if live.run != Some(pid) {
            return;
        }
        live.run = None;
        let ended_by_kernel = std::mem::replace(&mut live.run_ending, false);
        // Whatever it started goes with it.
        children::signal(pid, libc::SIGKILL);
        children::untrack(&self.db, pid);
        if !ended_by_kernel {
            let dir = self.cfg.proc_dir(name);
            if let Some(live) = self.live.get_mut(name) {
                live.run_down_at = Some(git::head(&dir).unwrap_or_default());
            }
            let output = tail(&fs::read_to_string(self.cfg.run_log(name)).unwrap_or_default(), 40);
            let addr = self.addr(name);
            self.note(&addr, "run_exit", json!({"status": status, "output": output}));
            if !ok {
                // The edge case goes to the agent.
                let notice = format!(
                    "[jarvis] Your .jarvis/run exited with {status}. It is started again once your code has changed.\n\nIts last output:\n{output}"
                );
                let me = self.addr(KERNEL);
                if let Err(e) = self.post(&me, &addr, &notice) {
                    self.trouble(name, &format!("could not tell it that its service exited: {e}"));
                }
            }
        }
        if self.live.get(name).is_some_and(|l| l.removing.is_some()) {
            self.finish_removal(name);
        } else {
            self.tend(name);
        }
    }

    /// If the process is idle and has mail, start a turn for it.
    fn dispatch(&mut self, proc: &Proc, dir: &Path) -> Res<()> {
        let name = proc.name.as_str();
        let live = &self.live[name];
        if live.busy || live.retry_at.is_some_and(|at| Instant::now() < at) {
            return Ok(());
        }
        let waiting: Vec<(String, String, String, i64)> = {
            let mut stmt = self.db.prepare_cached(
                "SELECT id, sender, body, attempts FROM messages WHERE recipient = ?1 AND status = 'pending' ORDER BY seq",
            )?;
            stmt.query_map([&proc.addr], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?
        };
        if waiting.is_empty() {
            return Ok(());
        }
        let mail: Vec<Mail> = waiting.into_iter().map(|(id, sender, body, attempts)| Mail { id, from: sender, body, attempt: attempts + 1 }).collect();
        let ids: Vec<&str> = mail.iter().map(|m| m.id.as_str()).collect();
        // From this record on the mail is handed out: all of it, or none of it.
        let began = self.rec(&proc.addr, "turn_start", json!({"messages": ids}))?;

        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            name: name.to_string(),
            addr: proc.addr.clone(),
            kernel: self.me.clone(),
            dir: dir.to_path_buf(),
            db_path: self.cfg.db_path(),
            harness: self.cfg.harness(&proc.harness),
            session: proc.session.clone(),
            env: self.proc_env(name),
            mail,
            note: git::not_committable(dir, &proc.branch),
            cancel: cancel.clone(),
            tx: self.tx.clone(),
        };
        let live = self.live.get_mut(name).expect("entry exists");
        live.busy = true;
        live.cancel = Some(cancel);
        live.turn = Some(began);
        live.turn_began_at = git::head(dir);
        let tx = self.tx.clone();
        let started = thread::Builder::new().spawn(move || {
            let _ = tx.send(Event::TurnDone(turn(job)));
        });
        if let Err(e) = started {
            self.release(name);
            self.rec(&proc.addr, "turn_end", json!({}))?;
            return Err(format!("could not start a turn: {e}").into());
        }
        Ok(())
    }

    fn release(&mut self, name: &str) {
        if let Some(live) = self.live.get_mut(name) {
            live.busy = false;
            live.cancel = None;
            live.turn = None;
        }
    }

    /// A turn is over.
    fn finish(&mut self, done: Done) {
        let name = done.name.clone();
        if let Err(e) = self.record(done) {
            self.trouble(&name, &format!("could not write down a finished turn: {e}"));
            // Whatever was written, the process is no longer mid-turn and its mail waits again.
            let addr = self.addr(&name);
            self.note(&addr, "turn_end", json!({}));
        }
        self.release(&name);
        if self.live.get(&name).is_some_and(|l| l.removing.is_some()) {
            self.finish_removal(&name);
        } else {
            self.tend(&name);
        }
    }

    /// Write down how a turn went, and commit what the process changed.
    fn record(&mut self, done: Done) -> Res<()> {
        let name = done.name.as_str();
        let proc = self.find(name)?.ok_or("the process is no longer there")?;
        let addr = proc.addr.as_str();
        if !done.by_code.is_empty() {
            self.rec(addr, "handled_by_code", json!({"messages": done.by_code}))?;
        }
        let mut failures = 0;
        let subject: String = match done.agent {
            Agent::NotNeeded => {
                self.rec(addr, "turn_end", json!({}))?;
                "handled by code".into()
            }
            Agent::Replied(mut out) => {
                let reply = out["reply"].as_str().unwrap_or("").to_string();
                // Everything the harness reported (cost, duration, ...) is kept.
                out["messages"] = json!(done.for_agent);
                self.rec(addr, "turn", out)?;
                reply.lines().find(|l| !l.trim().is_empty()).unwrap_or("turn").chars().take(72).collect()
            }
            Agent::Interrupted => {
                self.rec(addr, "turn_interrupted", json!({"messages": done.for_agent}))?;
                "turn interrupted".into()
            }
            Agent::Failed(error) => {
                // The mail waits and is offered again after a pause.
                failures = self.live.get(name).map_or(0, |l| l.failures) + 1;
                let delay = retry_delay(failures);
                self.rec(
                    addr,
                    "turn_failed",
                    json!({"messages": done.for_agent, "error": error, "failures": failures, "retry_in_s": delay.as_secs()}),
                )?;
                if let Some(live) = self.live.get_mut(name) {
                    live.retry_at = Some(Instant::now() + delay);
                }
                self.tell_senders(&proc, &done.for_agent, &error)?;
                "turn failed".into()
            }
        };
        if let Some(live) = self.live.get_mut(name) {
            live.failures = failures;
        }

        let dir = self.cfg.proc_dir(name);
        if let Some(why) = git::not_committable(&dir, &proc.branch) {
            self.rec(addr, "commit_skipped", json!({"why": why}))?;
            return Ok(());
        }
        match git::commit_all(&dir, name, &format!("{name}: {subject}")) {
            Ok(Some(commit)) => {
                self.rec(addr, "commit", json!({"commit": commit, "subject": subject}))?;
            }
            Ok(None) => {}
            Err(e) => {
                self.rec(addr, "commit_failed", json!({"error": e.to_string()}))?;
            }
        }
        // Its code changed during the turn, whether the kernel committed it
        // just now or the agent did: a running service is stale, and a
        // stopped one gets another go.
        if self.live.get(name).is_some_and(|l| git::head(&dir) != l.turn_began_at) {
            self.end_run(name);
        }
        Ok(())
    }

    /// The first time a message could not be dealt with, whoever sent it is told.
    fn tell_senders(&mut self, proc: &Proc, messages: &[String], error: &str) -> Res<()> {
        for message in messages {
            let (sender, attempts): (String, i64) =
                self.db.query_row("SELECT sender, attempts FROM messages WHERE id = ?1", [message], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if attempts != 1 || name_of(&sender) == KERNEL || sender == proc.addr {
                continue;
            }
            let notice = format!(
                "[jarvis] Your message {message} to {} has not been dealt with yet: its agent could not run. \
                 The message waits there and its kernel keeps trying; `jarvis ps {}` shows how it is going.\n\nThe error:\n{}",
                proc.addr,
                proc.addr,
                tail(error, 15),
            );
            let me = self.addr(KERNEL);
            self.post(&me, &sender, &notice)?;
        }
        Ok(())
    }

    // --------------------------------------------------------- commands

    fn mutate(&mut self, mutation: Mutation, reply: Sender<Result<Value, String>>) {
        let answer = match mutation {
            Mutation::Spawn { args, by, cwd, prompt } => self.spawn(args, &by, &cwd, prompt),
            Mutation::SetStatus { who, alive, by } => self.set_status(&who, alive, &by),
            Mutation::Remove { who, keep_branch, by } => {
                // Answered once nothing of the process runs any more.
                match self.remove(&who, keep_branch, &by, reply.clone()) {
                    Ok(()) => return,
                    Err(e) => Err(e),
                }
            }
            Mutation::Tag { who, tags, by } => self.tag(&who, &tags, &by),
            Mutation::Send { from, to, body } => self.send(&from, &to, &body),
            Mutation::Link { spec, remove } => self.link(&spec, remove),
            Mutation::CloseKernel { name, by } => self.close_kernel(&name, &by),
            Mutation::Ship => self.store.ship(&self.db, &self.me).map(|count| said(vec![format!("wrote {count} record(s) to the store")], vec![])),
            Mutation::Remote { on, request } => {
                if !self.table.contains_key(&on) {
                    Err(format!("the kernel `{on}` cannot be reached from `{}` right now (`jarvis kernels` shows which can)", self.me).into())
                } else {
                    self.asked_n += 1;
                    let rid = format!("{}-{}", self.me, self.asked_n);
                    self.asked.insert(rid.clone(), (reply, Instant::now()));
                    self.route_to(&on, json!({"t": "req", "rid": rid, "src": self.me, "dst": on, "body": request}));
                    return;
                }
            }
        };
        let _ = reply.send(answer.map_err(|e| e.to_string()));
    }

    fn spawn(&mut self, args: cli::Spawn, by: &str, cwd: &str, prompt: Option<String>) -> Res<Value> {
        let name = args.name.as_str();
        index::check_name(name, "process")?;
        if name == HUMAN || name == KERNEL {
            return Err(format!("`{name}` is reserved; choose another name").into());
        }
        let addr = self.addr(name);
        let branch = addr.clone();
        let maker = self.label(by);
        if let Some(existing) = self.find(name)? {
            if existing.status == "alive" {
                return Err(format!("`{name}` is already running on the kernel `{}`; a kernel runs a name once", self.me).into());
            }
            // Spawning what is stopped starts it.
            let mut out = self.set_status(&[addr.clone()], true, by)?;
            out["o"] = json!([addr]);
            return Ok(out);
        }
        if self.cfg.repo.git(&["check-ref-format", "--branch", &branch]).is_err() {
            return Err(format!("`{name}` cannot be a name: git does not accept `{branch}` as a branch name").into());
        }
        let repo = self.cfg.repo.clone();
        let continued = repo.has_branch(&branch);
        if continued && (args.from.is_some() || args.tree.is_some()) {
            return Err(format!(
                "the branch `{branch}` exists, so `{name}` carries on from it here, and --from or --tree cannot be used as well. \
                 To start afresh under this name, delete that branch first"
            )
            .into());
        }
        // Where it starts from: anything git knows (an expert's branch, a
        // process's branch, a tag, a commit), then a process by name, then a
        // branch that so far exists only on the remote.
        let mut forked: Option<String> = None;
        let start = match &args.from {
            None => None,
            Some(what) => {
                if let Some(commit) = repo.commit_of(what) {
                    if what.contains('@') {
                        forked = Some(what.clone());
                    }
                    Some(commit)
                } else if let Some(proc) = self.find(what)? {
                    let commit = repo.commit_of(&proc.branch).ok_or_else(|| format!("the branch of {} is not in this repository", proc.addr))?;
                    forked = Some(proc.addr);
                    Some(commit)
                } else if let Some(commit) = repo.commit_of(&format!("origin/{what}")) {
                    Some(commit)
                } else {
                    return Err(format!(
                        "`{what}` is neither a branch, tag or commit that git knows here, nor the name of a process on the kernel `{}` (`jarvis ps` lists them; one on another kernel is `NAME@KERNEL`)",
                        self.me
                    )
                    .into());
                }
            }
        };
        let tree = args.tree.as_ref().map(|tree| Path::new(cwd).join(tree));
        if let Some(tree) = &tree {
            if !tree.is_dir() {
                return Err(format!("{} is not a directory on the machine of the kernel `{}`", tree.display(), self.me).into());
            }
            if tree.join(".git").exists() {
                return Err(format!(
                    "{} is the top of a git repository, and copying it would copy its `.git` too. \
                     Use --tree on a directory inside it, or bring its history in with git and use --from.",
                    tree.display()
                )
                .into());
            }
        }
        let blank = prompt.is_none() && start.is_none() && tree.is_none() && !continued;
        let tags: serde_json::Map<String, Value> =
            args.tags.iter().map(|t| serve::parse_tag(t).map(|(k, v)| (k, Value::String(v)))).collect::<Res<_>>()?;

        let dir = self.cfg.proc_dir(name);
        if dir.exists() {
            // Left behind by a process of this name that was not fully removed.
            let _ = git::worktree_remove(&repo, &dir);
            let _ = fs::remove_dir_all(&dir);
        }
        git::worktree_add(&repo, &dir, &branch, match (&start, continued) {
            (_, true) => git::Start::Existing,
            (Some(commit), false) => git::Start::From(commit),
            (None, false) => git::Start::Nothing,
        })?;

        // From here on a failure must not leave half a process behind.
        let fill = |kernel: &mut Kernel| -> Res<()> {
            if let Some(tree) = &tree {
                copy_tree(tree, &dir).map_err(|e| format!("could not copy {}: {e}", tree.display()))?;
            }
            if let Some(prompt) = &prompt {
                fs::create_dir_all(dir.join(".jarvis"))?;
                fs::write(dir.join(".jarvis/PROMPT.md"), format!("{}\n", prompt.trim_end()))?;
            }
            let message = format!("{name}: {} on {} by {maker}", if continued { "carried on" } else { "spawned" }, kernel.me);
            if git::commit_all(&dir, name, &message)?.is_none() && !continued {
                git::commit_empty(&dir, name, &message)?;
            }
            kernel.rec(
                &addr,
                "spawn",
                json!({"parent": by, "from": args.from, "tree": args.tree, "harness": args.harness, "branch": branch,
                       "continued": continued, "tags": tags}),
            )?;
            Ok(())
        };
        if let Err(e) = fill(self) {
            let _ = git::worktree_remove(&repo, &dir);
            if !continued {
                let _ = repo.git(&["branch", "-D", &branch]);
            }
            return Err(format!("could not create `{name}`: {e}").into());
        }
        self.live.remove(name);

        // A first message, from whoever made it, wakes it up. A blank process
        // gets none: it waits for someone to tell it what to be.
        if !blank {
            let has_prompt = dir.join(".jarvis/PROMPT.md").exists();
            let read = if has_prompt { "Your instructions are in .jarvis/PROMPT.md. Read them and begin." } else { "Look at what is in your directory and begin." };
            // Said when it starts from a branch that is not a process: an expert.
            let is_branch = |what: &str| ["refs/heads", "refs/remotes/origin"].iter().any(|place| repo.git(&["rev-parse", "--verify", "--quiet", &format!("{place}/{what}")]).is_ok());
            let hello = if continued {
                format!(
                    "[jarvis] You have been started on the kernel `{me}` by {maker}, carrying on from your branch `{branch}`. \
                     Your directory is as that branch was. The conversation you had before did not come along: your notes in \
                     `.jarvis/` and `jarvis messages {name}` hold what was said. Look at both, and carry on.",
                    me = self.me
                )
            } else {
                match (&forked, &args.from) {
                    (Some(origin), _) if name_of(origin) == name => format!(
                        "[jarvis] You have been started on the kernel `{me}` by {maker}, as a continuation of `{origin}`: your directory \
                         began as a copy of its branch, notes included. You are `{addr}` now, with a branch of your own. The conversation \
                         `{origin}` had did not come along: your notes in `.jarvis/` and `jarvis messages {name}` hold what was said. \
                         Look at both, and carry on.",
                        me = self.me
                    ),
                    (Some(origin), _) => format!(
                        "[jarvis] You have just been created by {maker} as a fork of `{origin}`: your directory began as a copy of its \
                         branch, notes and prompt included, but you are a new process, `{addr}`, with a memory of your own. {}",
                        if prompt.is_some() { read.to_string() } else { format!("You were given no instructions of your own. Look at what is in your directory, and ask {maker} what you are for.") }
                    ),
                    (None, Some(what)) if is_branch(what) => format!(
                        "[jarvis] You have just been created by {maker}. Your directory began as a copy of the branch `{what}`: that is \
                         your expert, and where your finished work goes back to (the guide says how). {read}"
                    ),
                    (None, Some(what)) => format!("[jarvis] You have just been created by {maker}. Your directory began as a copy of `{what}`. {read}"),
                    (None, None) => format!("[jarvis] You have just been created by {maker}. {read}"),
                }
            };
            self.post(by, &addr, &hello)?;
        }
        let mut notes = Vec::new();
        if !is_executable(&self.cfg.harness(&args.harness)) {
            notes.push(format!(
                "there is no harness adapter `harness/{}` in {}, so its agent cannot run until there is",
                args.harness,
                self.cfg.repo.root.display()
            ));
        }
        if blank {
            notes.push(format!("`{name}` is blank: it does nothing until it gets a message (`jarvis send {addr} ...`)"));
        }
        self.advertise();
        self.tend(name);
        Ok(said(vec![addr], notes))
    }

    /// A process of this kernel, from what a command called it.
    fn local(&self, who: &str) -> Res<Proc> {
        let name = name_of(who);
        if who.contains('@') && kernel_of(who) != self.me {
            return Err(format!("`{who}` is not on the kernel `{}`", self.me).into());
        }
        self.find(name)?.ok_or_else(|| {
            let elsewhere: Vec<String> = index::procs_named(&self.db, name).unwrap_or_default().into_iter().map(|p| p.addr).collect();
            if elsewhere.is_empty() {
                format!("there is no process called `{name}` on the kernel `{}` (`jarvis ps` lists them)", self.me).into()
            } else {
                format!("there is no process called `{name}` on the kernel `{}`. Did you mean {}?", self.me, elsewhere.join(" or ")).into()
            }
        })
    }

    fn set_status(&mut self, who: &[String], alive: bool, by: &str) -> Res<Value> {
        // All of them must exist before any is touched.
        let procs: Vec<Proc> = who.iter().map(|who| self.local(who)).collect::<Res<_>>()?;
        let word = if alive { "alive" } else { "stopped" };
        let mut lines = Vec::new();
        for proc in procs {
            if proc.status == word {
                lines.push(format!("{} was already {word}", proc.name));
                continue;
            }
            self.rec(&proc.addr, if alive { "start" } else { "stop" }, json!({"by": by}))?;
            lines.push(format!("{} is {word}", proc.name));
            self.tend(&proc.name);
        }
        Ok(said(lines, vec![]))
    }

    fn tag(&mut self, who: &str, tags: &[(String, String)], by: &str) -> Res<Value> {
        let proc = self.local(who)?;
        let set: serde_json::Map<String, Value> = tags.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        self.rec(&proc.addr, "tag", json!({"by": by, "set": set}))?;
        Ok(said(vec![], vec![]))
    }

    /// Begin removing a process. It is finished, and answered, once nothing
    /// of the process runs any more.
    fn remove(&mut self, who: &str, keep_branch: bool, by: &str, reply: Sender<Result<Value, String>>) -> Res<()> {
        let proc = self.local(who)?;
        if proc.addr == by {
            return Err("a process cannot remove itself: this command is one of the programs that would be ended. Ask another process, or the human".into());
        }
        let live = self.live.entry(proc.name.clone()).or_default();
        if live.removing.is_some() {
            return Err(format!("{} is being removed already", proc.name).into());
        }
        live.removing = Some(Removal { keep_branch, by: by.to_string(), reply });
        self.put_to_rest(&proc.name);
        self.finish_removal(&proc.name);
        Ok(())
    }

    fn finish_removal(&mut self, name: &str) {
        let Some(live) = self.live.get(name) else { return };
        if live.busy || live.run.is_some() {
            return;
        }
        let Some(removal) = self.live.remove(name).and_then(|l| l.removing) else { return };
        let done = (|| -> Res<Value> {
            let proc = self.find(name)?.ok_or("it is no longer there")?;
            let repo = self.cfg.repo.clone();
            let dir = self.cfg.proc_dir(name);
            let commit = repo.git(&["rev-parse", "--short", &proc.branch]).ok();
            let (turns, cost): (i64, Option<f64>) = self.db.query_row(
                "SELECT COUNT(*), SUM(json_extract(data, '$.cost_usd')) FROM records WHERE proc = ?1 AND kind = 'turn'",
                [&proc.addr],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if dir.exists() {
                git::worktree_remove(&repo, &dir).map_err(|e| format!("could not delete the directory of {name}, so it was not removed: {e}"))?;
            }
            if !removal.keep_branch && commit.is_some() {
                repo.git(&["branch", "-D", &proc.branch])?;
            }
            let _ = fs::remove_file(self.cfg.run_log(name));
            self.rec(
                &proc.addr,
                "remove",
                json!({"by": removal.by, "commit": commit, "branch_kept": removal.keep_branch, "turns": turns, "cost_usd": cost}),
            )?;
            self.advertise();
            Ok(said(
                vec![match (&commit, removal.keep_branch) {
                    (Some(commit), true) => format!("removed {name}; its branch `{}` stays, at {commit}", proc.branch),
                    (Some(commit), false) => format!(
                        "removed {name} (its branch was at {commit}; `git branch {} {commit}` brings its files back while git still has them)",
                        proc.branch
                    ),
                    (None, _) => format!("removed {name}"),
                }],
                vec![],
            ))
        })();
        let _ = removal.reply.send(done.map_err(|e| e.to_string()));
    }

    fn close_kernel(&mut self, name: &str, by: &str) -> Res<Value> {
        if name == self.me {
            return Err("that is this kernel, and it is running. Ctrl-C where it runs stops it".into());
        }
        let known: bool = self.db.query_row("SELECT COUNT(*) FROM kernels WHERE name = ?1", [name], |r| r.get::<_, i64>(0))? > 0;
        if !known {
            return Err(format!("no kernel called `{name}` is known here (`jarvis kernels` lists them)").into());
        }
        let me = self.addr(KERNEL);
        self.rec(&me, "closed", json!({"kernel": name, "by": by}))?;
        Ok(said(vec![format!("{name} is taken to be gone")], vec![]))
    }

    // --------------------------------------------------------- messages

    /// Turn what someone typed after `jarvis send` into a full address, or say
    /// why it cannot be sent there.
    fn resolve_to(&self, to: &str) -> Res<String> {
        let (name, kernel) = (name_of(to), kernel_of(to));
        if !kernel.is_empty() {
            if kernel == self.me {
                if !index::is_person(to) && self.find(name)?.is_none() {
                    return Err(format!("there is no process called `{name}` on the kernel `{}` (`jarvis ps` lists them)", self.me).into());
                }
            } else if !self.table.contains_key(kernel) && index::kernel_closed(&self.db, kernel) {
                return Err(format!("the kernel `{kernel}` has closed or is taken to be gone, so nothing can be delivered to `{to}` (`jarvis kernels` shows which are known)").into());
            }
            return Ok(to.to_string());
        }
        if matches!(name, HUMAN | KERNEL) || self.find(name)?.is_some() {
            return Ok(self.addr(name));
        }
        // A bare name is this kernel's process and nothing else.
        let elsewhere: Vec<String> = index::procs_named(&self.db, name)?.into_iter().map(|p| p.addr).collect();
        if elsewhere.is_empty() {
            Err(format!("there is no process called `{name}` on the kernel `{}` (`jarvis ps` lists them)", self.me).into())
        } else {
            Err(format!(
                "there is no process called `{name}` on the kernel `{}`. A process on another kernel needs its whole address: {}",
                self.me,
                elsewhere.join(" or ")
            )
            .into())
        }
    }

    fn send(&mut self, from: &str, to: &str, body: &str) -> Res<Value> {
        let to = self.resolve_to(to)?;
        let id = self.post(from, &to, body)?;
        let mut notes = Vec::new();
        // If it will not be read soon, say why.
        if kernel_of(&to) == self.me {
            if let Some(proc) = self.find(name_of(&to))? {
                if proc.status != "alive" {
                    notes.push(format!("{} is stopped; the message waits until `jarvis start {}`", proc.name, proc.name));
                } else if proc.failures > 0 {
                    notes.push(format!(
                        "{}'s last {} turn(s) failed; the message waits while the kernel keeps trying (`jarvis ps {}` shows why)",
                        proc.name, proc.failures, proc.name
                    ));
                }
            }
        } else if !self.table.contains_key(kernel_of(&to)) {
            notes.push(format!(
                "the kernel `{}` cannot be reached right now; the message waits here and goes on when it can",
                kernel_of(&to)
            ));
        }
        Ok(said(vec![format!("sent message {id} to {}", self.label(&to))], notes))
    }

    /// Send a message: write it down, and deliver it or start it on its way.
    /// `to` is a full address. Returns the message's name.
    fn post(&mut self, from: &str, to: &str, body: &str) -> Res<String> {
        let id = format!("{}:{}", self.me, self.next_n);
        let mut message = json!({"id": id, "from": from, "to": to, "body": body});
        // What its sender had been given when it wrote this: the turn it is in.
        if kernel_of(from) == self.me {
            if let Some(turn) = self.live.get(name_of(from)).and_then(|l| l.turn) {
                message["turn"] = json!(turn);
            }
        }
        self.rec(from, "send", message.clone())?;
        let dest = kernel_of(to);
        if dest == self.me {
            if !index::is_person(to) {
                self.tend(name_of(to));
            }
        } else {
            message["at"] = json!(index::now_ms());
            self.db.execute("INSERT OR IGNORE INTO outbox (id, dest, payload) VALUES (?1, ?2, ?3)", params![id, dest, message.to_string()])?;
            self.pump_outbox();
        }
        Ok(id)
    }

    /// Hand the messages held for other kernels to the links that lead there.
    fn pump_outbox(&mut self) {
        let held: Vec<(String, String, String)> = {
            let Ok(mut stmt) = self.db.prepare_cached("SELECT id, dest, payload FROM outbox") else { return };
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
        };
        for (id, dest, payload) in held {
            if self.awaiting.contains_key(&id) || self.paused.contains(&id) {
                continue;
            }
            let Some(link) = self.table.get(&dest).map(|k| k.link) else { continue };
            let Ok(mut message) = serde_json::from_str::<Value>(&payload) else { continue };
            message["hops"] = json!(message["hops"].as_i64().unwrap_or(0) + 1);
            if self.say(link, json!({"t": "msg", "m": message})) {
                self.awaiting.insert(id, link);
            }
        }
    }

    /// A message arrived over a link: it is for a process here, or to be passed on.
    fn receive(&mut self, link: u64, message: &Value) {
        let (Some(id), Some(to), Some(from)) = (message["id"].as_str(), message["to"].as_str(), message["from"].as_str()) else { return };
        let peer = self.links.get(&link).and_then(|l| l.peer.clone()).unwrap_or_default();
        let result = (|| -> Res<()> {
            if kernel_of(to) != self.me {
                // Kept until the next kernel on the way has confirmed it.
                let added = self.db.execute("INSERT OR IGNORE INTO outbox (id, dest, payload) VALUES (?1, ?2, ?3)", params![id, kernel_of(to), message.to_string()])?;
                if added > 0 && message["hops"].as_i64().unwrap_or(0) > MOST_HOPS {
                    // It is going in circles: the kernels disagree, for now, about the way.
                    self.paused.insert(id.to_string());
                    let mut fresh = message.clone();
                    fresh["hops"] = json!(0);
                    self.db.execute("UPDATE outbox SET payload = ?2 WHERE id = ?1", params![id, fresh.to_string()])?;
                }
                return Ok(());
            }
            // A message that arrives a second time (its confirmation was lost) arrives once.
            if self.db.execute("INSERT OR IGNORE INTO seen (id) VALUES (?1)", [id])? == 0 {
                return Ok(());
            }
            let mut arrived = message.clone();
            arrived["via"] = json!(peer);
            self.rec(to, "arrive", arrived)?;
            if index::is_person(to) {
                return Ok(());
            }
            if self.find(name_of(to))?.is_some() {
                self.tend(name_of(to));
                return Ok(());
            }
            let why = format!("there is no process called `{}` on the kernel `{}`", name_of(to), self.me);
            self.rec(to, "undeliverable", json!({"message": id, "why": why}))?;
            // Whoever sent it is told, unless that is a kernel telling someone something.
            if name_of(from) != KERNEL {
                let me = self.addr(KERNEL);
                self.post(&me, from, &format!("[jarvis] Your message {id} to {to} could not be delivered: {why}."))?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.say(link, json!({"t": "ack", "id": id}));
                self.pump_outbox();
            }
            // Not confirmed: the other kernel keeps it and offers it again.
            Err(e) => eprintln!("jarvis: could not take in the message {id}: {e}"),
        }
    }

    /// The next kernel has the message: this one need not keep it any longer.
    fn confirmed(&mut self, link: u64, id: &str) {
        if self.awaiting.get(id) != Some(&link) {
            return;
        }
        self.awaiting.remove(id);
        let payload: Option<String> = self.db.query_row("SELECT payload FROM outbox WHERE id = ?1", [id], |r| r.get(0)).ok();
        let _ = self.db.execute("DELETE FROM outbox WHERE id = ?1", [id]);
        let next = self.links.get(&link).and_then(|l| l.peer.clone()).unwrap_or_default();
        let from = payload.and_then(|p| serde_json::from_str::<Value>(&p).ok()).and_then(|m| m["from"].as_str().map(str::to_string)).unwrap_or_default();
        self.note(&from, "forward", json!({"message": id, "next": next}));
    }

    // ------------------------------------------------------------ links

    fn link(&mut self, spec: &str, remove: bool) -> Res<Value> {
        if remove {
            if self.db.execute("DELETE FROM links WHERE spec = ?1", [spec])? == 0 {
                return Err("this kernel has no such link (`jarvis link` lists them)".into());
            }
            if let Some(id) = self.dials.remove(spec).and_then(|d| d.link) {
                if let Some(link) = self.links.get(&id) {
                    (link.close)();
                }
            }
            self.publish_net();
            return Ok(said(vec!["the link is forgotten".into()], vec![]));
        }
        if let Some(kernel) = serde_json::from_str::<Value>(spec)?["kernel"].as_str() {
            if kernel == self.me {
                return Err("a kernel cannot be linked to itself".into());
            }
        }
        self.db.execute("INSERT OR IGNORE INTO links (spec) VALUES (?1)", [spec])?;
        self.dials.entry(spec.to_string()).or_insert(Dial { link: None, next_try: Instant::now(), wait: Duration::from_secs(2), problem: None });
        self.publish_net();
        Ok(said(vec!["the link is being made; `jarvis link` shows how it is going".into()], vec![]))
    }

    /// Send one line over a link. False if the link is not there.
    fn say(&self, link: u64, line: Value) -> bool {
        self.links.get(&link).is_some_and(|l| l.tx.send(line.to_string()).is_ok())
    }

    /// Send a line towards a kernel, over whichever link leads there.
    fn route_to(&self, kernel: &str, line: Value) -> bool {
        self.table.get(kernel).is_some_and(|k| self.say(k.link, line))
    }

    fn link_open(&mut self, id: u64, _spec: Option<String>, tx: Sender<String>, close: net::Close) {
        self.links.insert(id, Link { tx, close, peer: None, heard: Instant::now() });
        self.say(id, json!({"t": "hello", "name": self.me, "identity": self.cfg.identity}));
    }

    /// Say no to a link, and end it.
    fn refuse(&mut self, id: u64, why: &str) {
        self.say(id, json!({"t": "refuse", "why": why}));
        self.link_problem(id, why);
        // Give the line a moment to leave before the link is closed.
        thread::sleep(Duration::from_millis(100));
        if let Some(link) = self.links.get(&id) {
            (link.close)();
        }
    }

    fn link_problem(&mut self, id: u64, why: &str) {
        if let Some(dial) = self.dials.values_mut().find(|d| d.link == Some(id)) {
            dial.problem = Some(why.to_string());
        }
        let me = self.addr(KERNEL);
        self.note(&me, "link_refused", json!({"why": why}));
    }

    fn link_line(&mut self, id: u64, v: Value) {
        let Some(link) = self.links.get_mut(&id) else { return };
        link.heard = Instant::now();
        let has_peer = link.peer.is_some();
        match v["t"].as_str().unwrap_or("") {
            "hello" => {
                let (name, identity) = (v["name"].as_str().unwrap_or("").to_string(), v["identity"].as_str().unwrap_or(""));
                if name.is_empty() || has_peer {
                    return;
                }
                if name == self.me {
                    return self.refuse(id, &format!("both kernels are called `{name}`. Two kernels of one name cannot be linked"));
                }
                if self.table.get(&name).is_some_and(|k| k.identity != identity) {
                    return self.refuse(id, &format!("another kernel called `{name}` can already be reached from `{}`. Two kernels of one name cannot be linked", self.me));
                }
                if let Some(link) = self.links.get_mut(&id) {
                    link.peer = Some(name.clone());
                }
                if let Some(dial) = self.dials.values_mut().find(|d| d.link == Some(id)) {
                    dial.problem = None;
                    dial.wait = Duration::from_secs(2);
                }
                let me = self.addr(KERNEL);
                self.note(&me, "link_up", json!({"kernel": name}));
                self.advertise();
            }
            "refuse" => {
                let why = v["why"].as_str().unwrap_or("the other kernel refused the link").to_string();
                self.link_problem(id, &why);
                if let Some(link) = self.links.get(&id) {
                    (link.close)();
                }
            }
            "table" if has_peer => self.learn(id, &v["kernels"]),
            "msg" if has_peer => self.receive(id, &v["m"]),
            "ack" => self.confirmed(id, v["id"].as_str().unwrap_or("")),
            "req" => {
                let (rid, src, dst) = (v["rid"].as_str().unwrap_or("").to_string(), v["src"].as_str().unwrap_or("").to_string(), v["dst"].as_str().unwrap_or(""));
                if dst == self.me {
                    // Carried out on a thread of its own, like a command from here.
                    let (cfg, tx, shared, body) = (self.cfg.clone(), self.tx.clone(), self.shared.clone(), v["body"].clone());
                    thread::spawn(move || {
                        let body = serve::answer(cfg, tx.clone(), shared, body);
                        let _ = tx.send(Event::Answered { rid, to: src, body });
                    });
                } else if !self.route_to(dst, v.clone()) {
                    let body = json!({"o": [], "e": [format!("jarvis: the kernel `{dst}` cannot be reached from `{}`", self.me)], "code": 1});
                    self.route_to(&src, json!({"t": "res", "rid": rid, "dst": src, "body": body}));
                }
            }
            "res" => {
                let dst = v["dst"].as_str().unwrap_or("");
                if dst == self.me {
                    if let Some((reply, _)) = self.asked.remove(v["rid"].as_str().unwrap_or("")) {
                        let _ = reply.send(Ok(v["body"].clone()));
                    }
                } else {
                    self.route_to(dst, v.clone());
                }
            }
            _ => {}
        }
    }

    fn link_down(&mut self, id: u64, why: &str) {
        let was = self.links.remove(&id);
        if let Some(link) = &was {
            (link.close)();
        }
        if let Some(dial) = self.dials.values_mut().find(|d| d.link == Some(id)) {
            dial.link = None;
            dial.next_try = Instant::now() + dial.wait;
            dial.wait = (dial.wait * 2).min(Duration::from_secs(60));
            // Why a link that was refused went down is known already.
            if dial.problem.is_none() || was.as_ref().is_some_and(|l| l.peer.is_some()) {
                dial.problem = Some(why.to_string());
            }
        }
        // What it was asked to carry and has not confirmed is offered again.
        self.awaiting.retain(|_, link| *link != id);
        if let Some(peer) = was.and_then(|l| l.peer) {
            let me = self.addr(KERNEL);
            self.note(&me, "link_down", json!({"kernel": peer, "why": why}));
        }
        self.offers.remove(&id);
        self.choose_routes();
        self.publish_net();
    }

    /// A neighbour said which kernels it can reach. Take over what is new or
    /// nearer, drop what it no longer offers.
    fn learn(&mut self, id: u64, kernels: &Value) {
        let Some(entries) = kernels.as_array() else { return };
        let mut offered: HashMap<String, Known> = HashMap::new();
        for e in entries {
            let (Some(name), Some(identity)) = (e["name"].as_str(), e["identity"].as_str()) else { continue };
            let hops = e["hops"].as_u64().unwrap_or(FARTHEST) + 1;
            if name == self.me {
                if identity != self.cfg.identity {
                    return self.refuse(id, &format!("another kernel called `{name}` can be reached through this link. Two kernels of one name cannot be linked"));
                }
                continue;
            }
            if self.table.get(name).is_some_and(|k| k.identity != identity) {
                return self.refuse(id, &format!("two different kernels called `{name}` would be reachable. Two kernels of one name cannot be linked"));
            }
            if hops > FARTHEST {
                continue;
            }
            let procs = e["procs"].as_array().map(|a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()).unwrap_or_default();
            offered.insert(name.to_string(), Known { identity: identity.to_string(), hops, link: id, procs });
        }
        self.offers.insert(id, offered);
        self.choose_routes();
    }

    /// For every kernel a neighbour offers, take the nearest way. Every
    /// offer is kept, so that when a link goes, a longer way that was there
    /// all along is used at once.
    fn choose_routes(&mut self) {
        let mut best: HashMap<String, Known> = HashMap::new();
        let mut links: Vec<&u64> = self.offers.keys().collect();
        links.sort();
        for link in links {
            for (name, offer) in &self.offers[link] {
                if best.get(name).is_none_or(|b| offer.hops < b.hops) {
                    best.insert(name.clone(), Known { identity: offer.identity.clone(), hops: offer.hops, link: *link, procs: offer.procs.clone() });
                }
            }
        }
        let same = best.len() == self.table.len()
            && best.iter().all(|(name, new)| self.table.get(name).is_some_and(|old| old.link == new.link && old.hops == new.hops && old.procs == new.procs));
        if !same {
            self.table = best;
            self.routes_changed();
        }
    }

    /// The kernels that can be reached changed: tell the neighbours, and try
    /// again what was waiting for a way.
    fn routes_changed(&mut self) {
        self.paused.clear();
        self.advertise();
        self.pump_outbox();
    }

    /// Tell every neighbour which kernels can be reached through this one,
    /// and which processes each has. (Not those reached through the
    /// neighbour itself: it knows a shorter way to them.)
    fn advertise(&mut self) {
        let mine: Vec<String> = index::procs(&self.db, true).unwrap_or_default().into_iter().filter(|p| p.kernel == self.me).map(|p| p.name).collect();
        for (id, link) in &self.links {
            if link.peer.is_none() {
                continue;
            }
            let mut kernels = vec![json!({"name": self.me, "identity": self.cfg.identity, "hops": 0, "procs": mine})];
            for (name, k) in &self.table {
                if k.link != *id {
                    kernels.push(json!({"name": name, "identity": k.identity, "hops": k.hops, "procs": k.procs}));
                }
            }
            let _ = link.tx.send(json!({"t": "table", "kernels": kernels}).to_string());
        }
        self.publish_net();
    }

    /// Put what is known about kernels and links where commands can read it.
    fn publish_net(&self) {
        let peer_of = |id: u64| self.links.get(&id).and_then(|l| l.peer.clone());
        let mut reachable: Vec<Reachable> = self
            .table
            .iter()
            .map(|(name, k)| Reachable { name: name.clone(), hops: k.hops, via: peer_of(k.link).unwrap_or_default(), procs: k.procs.clone() })
            .collect();
        reachable.sort_by(|a, b| (a.hops, &a.name).cmp(&(b.hops, &b.name)));
        let mut links: Vec<LinkView> = self
            .dials
            .iter()
            .map(|(spec, d)| LinkView { spec: Some(spec.clone()), peer: d.link.and_then(peer_of), problem: d.problem.clone() })
            .collect();
        let dialled: HashSet<u64> = self.dials.values().filter_map(|d| d.link).collect();
        links.extend(self.links.iter().filter(|(id, _)| !dialled.contains(id)).map(|(_, l)| LinkView { spec: None, peer: l.peer.clone(), problem: None }));
        *self.shared.net.lock().unwrap() = NetView { reachable, links };
    }
}
