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
//! 4. passes on what is for other kernels over its links (messages, requests,
//!    and what kernels tell each other about them), keeping each thing until
//!    the next kernel has confirmed it;
//! 5. writes down everything that happens to it as records (`index.rs`), and
//!    answers the questions other kernels ask about them.
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
//!   kernel is kept here until the next kernel on the way has it, and the
//!   kernel it was for says back how far it got.
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

mod links;
mod mail;
mod procs;
mod turn;

pub use links::link_words;
use turn::turn;

/// How long a program gets to end by itself after being asked to.
const GRACE: Duration = Duration::from_secs(3);
/// A message that went through this many kernels is going in circles.
const MOST_HOPS: i64 = 32;
/// A kernel further away than this many links counts as out of reach.
const FARTHEST: u64 = 16;
/// How long a question to another kernel may go unanswered.
const ASK_PATIENCE: Duration = Duration::from_secs(30);

/// Where a kernel is and what it uses. Fixed while it runs.
pub struct Config {
    pub name: String,
    pub identity: String,
    pub repo: Repo,
    /// The kernel's own directory: its database, its processes' directories.
    pub state: PathBuf,
    /// Where the kernels of this machine put their sockets.
    pub run_dir: PathBuf,
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

    /// The adapter that runs a process's agent; none for a process without one.
    fn harness(&self, name: &str) -> Option<PathBuf> {
        (name != NO_AGENT).then(|| self.repo.root.join("harness").join(name))
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

/// The harness of a process that has no agent: its code deals with its mail.
pub const NO_AGENT: &str = "none";

/// What the kernel shares with the threads that answer commands.
pub struct Shared {
    /// Whoever follows the records (`jarvis watch`, `jarvis log -f`) is told
    /// here that there are new ones, or that the kernels in reach changed.
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
    pub version: String,
}

#[derive(Clone)]
pub struct LinkView {
    pub spec: Option<String>,
    pub peer: Option<String>,
    pub problem: Option<String>,
    /// How often it was tried in a row without coming up, and in how many
    /// seconds it is tried next.
    pub tries: u32,
    pub next_try_s: Option<u64>,
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
    /// A question another kernel asked this one has an answer.
    Answered { rid: String, to: String, body: Value },
    /// A request that was sent to this kernel has been carried out.
    RequestDone { id: String, body: Value },
    /// Something for a kernel that follows this one: a line, or the end.
    Publish { sid: String, to: String, item: Value },
    Tick,
}

/// What a command asks the kernel for. The answer is `{"o": [lines], "e": [notes]}`.
pub enum Mutation {
    Spawn { args: cli::Spawn, by: String, prompt: Option<String> },
    SetStatus { who: Vec<String>, alive: bool, by: String },
    Remove { who: String, keep_branch: bool, by: String },
    Tag { who: String, tags: Vec<(String, String)>, by: String },
    Send { from: String, to: String, body: String },
    Link { spec: String, remove: bool },
    CloseKernel { name: String, by: String },
    /// Have another kernel carry out a command, however long it takes to get
    /// there. Answered at once with the request's name; what came of it is
    /// given to `outcome` when that kernel has said.
    Request { kernel: String, by: String, argv: Vec<String>, extra: Value, outcome: Sender<Value> },
    /// Ask another kernel a question, now. It fails if that kernel cannot be reached.
    Ask { on: String, request: Value },
    /// Follow another kernel: what the command prints there arrives on `to`,
    /// until either side ends it. Answered with the name of the subscription.
    Subscribe { on: String, request: Value, to: Sender<Value> },
    Unsubscribe { sid: String },
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
    /// The adapter that runs its agent; none if it has no agent.
    harness: Option<PathBuf>,
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
    /// The process has no agent, and its code did not take these messages.
    NoAgent,
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

// ----------------------------------------------------------------- kernel

/// A link to another kernel, while it is up.
struct Link {
    tx: Sender<String>,
    close: net::Close,
    /// The kernel at the other end, once it has said who it is.
    peer: Option<String>,
    heard: Instant,
    /// One side said no to it; why is known already.
    refused: bool,
}

/// A link this kernel was told to keep (`jarvis link`), up or not.
struct Dial {
    link: Option<u64>,
    next_try: Instant,
    wait: Duration,
    /// Why it is down, as of the last try.
    problem: Option<String>,
    /// Tries in a row that did not bring it up.
    tries: u32,
}

impl Dial {
    fn new() -> Dial {
        Dial { link: None, next_try: Instant::now(), wait: Duration::from_secs(2), problem: None, tries: 0 }
    }
}

/// What is known of a kernel that can be reached over links.
struct Known {
    identity: String,
    hops: u64,
    link: u64,
    version: String,
}

pub struct Kernel {
    cfg: Arc<Config>,
    me: String,
    db: Connection,
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
    /// Questions put to other kernels that have not been answered yet.
    asked: HashMap<String, (Sender<Result<Value, String>>, Instant)>,
    asked_n: u64,
    /// Commands that wait to hear what came of a request they sent to another
    /// kernel, by the request's name.
    waiters: HashMap<String, Sender<Value>>,
    /// Other kernels this one follows on someone's behalf: for each
    /// subscription, the kernel and where its lines go.
    subs: HashMap<String, (String, Sender<Value>)>,
    /// Kernels that follow this one: for each subscription, who, and the
    /// switch that ends the command that feeds it.
    pubs: HashMap<String, (String, Arc<AtomicBool>)>,
    last_ping: Instant,
}

/// `jarvis up`: set a kernel up, run it until it is told to stop.
pub fn up(args: cli::Up) -> Res<()> {
    // A link's command or a client can end while the kernel is writing to
    // it. That must fail that one write, not end the kernel (which is what a
    // broken pipe does to a command-line program, and what `main` asks for).
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    let name = args.name.clone();
    index::check_name(&name, "kernel")?;
    if matches!(name.as_str(), HUMAN | KERNEL) {
        return Err(format!("`{name}` is reserved; choose another name for the kernel").into());
    }
    let here = std::env::current_dir()?;
    let absolute = |text: &str| -> PathBuf { here.join(text) };
    let repo = Repo::find(&args.repo.as_deref().map(absolute).unwrap_or(here.clone()))?;
    let state = args.state.as_deref().map(absolute).unwrap_or_else(|| repo.root.join(".jarvis/kernel").join(&name));
    for dir in [state.join("proc"), state.join("log")] {
        fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    // What the kernel keeps inside the repository's directory is not code.
    hide_from_git(&repo, &[&state]);

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
    // The name is taken in the repository before anything is written under
    // it. That refuses a second kernel of the same name.
    repo.claim(&name, &identity)?;
    index::set_meta(&db, "identity", &identity)?;

    let cfg = Arc::new(Config {
        name: name.clone(),
        identity,
        repo,
        state,
        run_dir,
    });
    fs::write(cfg.guide(), include_str!("../../GUIDE.md"))?;

    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Shared { followers: Mutex::new(Vec::new()), net: Mutex::new(NetView::default()), next_link: AtomicU64::new(1) });
    // Being the one kernel of this name on this machine is having its socket.
    serve::listen(cfg.clone(), tx.clone(), shared.clone())?;

    let next_n: i64 = db.query_row("SELECT COALESCE(MAX(n), 0) + 1 FROM records WHERE k = ?1", [&name], |r| r.get(0))?;
    let printed: i64 = db.query_row("SELECT COALESCE(MAX(seq), 0) FROM records", [], |r| r.get(0))?;
    let mut kernel = Kernel {
        cfg,
        me: name,
        db,
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
        waiters: HashMap::new(),
        subs: HashMap::new(),
        pubs: HashMap::new(),
        last_ping: Instant::now(),
    };
    if new {
        let identity = kernel.cfg.identity.clone();
        kernel.rec(&at(KERNEL, &kernel.me.clone()), "kernel", json!({"identity": identity, "version": cli::VERSION}))?;
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
            "jarvis: kernel `{}` is up (jarvis {}). Code in {}, its own state in {}, its socket in {}. Ctrl-C stops it.",
            self.me,
            cli::VERSION,
            self.cfg.repo.root.display(),
            self.cfg.state.display(),
            self.cfg.run_dir.display()
        );
        let me = self.addr(KERNEL);
        self.rec(&me, "up", json!({"pid": std::process::id(), "version": cli::VERSION}))?;
        let links: Vec<String> = {
            let mut stmt = self.db.prepare("SELECT spec FROM links")?;
            stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for spec in links {
            self.dials.insert(spec, Dial::new());
        }
        for proc in index::procs(&self.db)? {
            self.tend(&proc.name);
        }
        // Requests that arrived and were not carried out before the kernel last stopped.
        let left: Vec<String> = {
            let mut stmt = self.db.prepare("SELECT data FROM records WHERE kind = 'arrive' AND json_extract(data, '$.id') IN (SELECT id FROM messages WHERE recipient = ?1 AND status = 'pending')")?;
            stmt.query_map([&me], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        for data in left {
            if let Ok(message) = serde_json::from_str::<Value>(&data) {
                self.carry_out(&message);
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
                self.wake_followers();
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
            Event::RequestDone { id, body } => self.request_done(&id, &body),
            Event::Publish { sid, to, item } => {
                let mut line = json!({"t": "pub", "sid": sid, "src": self.me, "dst": to});
                for (key, value) in item.as_object().into_iter().flatten() {
                    line[key] = value.clone();
                }
                let ended = item.get("end").is_some();
                // Nobody to tell any more: the command that feeds it ends.
                if !self.route_to(&to, line) || ended {
                    self.end_pub(&sid);
                }
            }
            Event::Tick => self.tick(),
        }
    }

    /// Tell whoever follows this kernel to look again.
    fn wake_followers(&self) {
        self.shared.followers.lock().unwrap().retain(|f| !matches!(f.try_send(()), Err(mpsc::TrySendError::Disconnected(_))));
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

    /// Stop: end every program the kernel started, and say so. Mail that
    /// was being worked on is given again next time.
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
        self.note(&me, "closed", json!({}));
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
        // A question another kernel never answered.
        let stale: Vec<String> = self.asked.iter().filter(|(_, (_, since))| now.duration_since(*since) > ASK_PATIENCE).map(|(rid, _)| rid.clone()).collect();
        for rid in stale {
            if let Some((reply, _)) = self.asked.remove(&rid) {
                let _ = reply.send(Err(format!("it did not answer within {} seconds", ASK_PATIENCE.as_secs())));
            }
        }

    }

    /// Print what was written down since last time, as `jarvis log` would.
    fn print_new(&mut self) {
        match serve::fact_lines(&self.db, &mut self.printed) {
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

    // --------------------------------------------------------- commands

    fn mutate(&mut self, mutation: Mutation, reply: Sender<Result<Value, String>>) {
        let answer = match mutation {
            Mutation::Spawn { args, by, prompt } => self.spawn(args, &by, prompt),
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
            Mutation::Request { kernel, by, argv, extra, outcome } => self.request(&kernel, &by, &argv, extra, outcome),
            Mutation::Ask { on, request } => {
                if !self.table.contains_key(&on) {
                    Err(self.out_of_reach(&on).into())
                } else {
                    self.asked_n += 1;
                    let rid = format!("{}-{}", self.me, self.asked_n);
                    self.asked.insert(rid.clone(), (reply, Instant::now()));
                    self.route_to(&on, json!({"t": "req", "rid": rid, "src": self.me, "dst": on, "body": request}));
                    return;
                }
            }
            Mutation::Subscribe { on, request, to } => {
                if !self.table.contains_key(&on) {
                    Err(self.out_of_reach(&on).into())
                } else {
                    self.asked_n += 1;
                    let sid = format!("{}-{}", self.me, self.asked_n);
                    self.subs.insert(sid.clone(), (on.clone(), to));
                    self.route_to(&on, json!({"t": "sub", "sid": sid, "src": self.me, "dst": on, "body": request}));
                    Ok(said(vec![sid], vec![]))
                }
            }
            Mutation::Unsubscribe { sid } => {
                if let Some((on, _)) = self.subs.remove(&sid) {
                    self.route_to(&on, json!({"t": "unsub", "sid": sid, "dst": on}));
                }
                Ok(said(vec![], vec![]))
            }
        };
        let _ = reply.send(answer.map_err(|e| e.to_string()));
    }
}
