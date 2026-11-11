//! The kernel. It does four things and holds no opinion about any of them:
//!
//! 1. keeps each process's `.jarvis/run` going, if the process has one;
//! 2. hands waiting messages to the process: first to its `.jarvis/handle`,
//!    if it has one, otherwise (or when the handler declines or fails) to its
//!    agent;
//! 3. commits whatever the process changed in its own directory;
//! 4. records what happened.
//!
//! What a process is for, how it is built, what it needs, who it talks to and
//! how many of them there are is none of the kernel's business.
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
//! key is something the agent did, and is recorded at once. The last line,
//! {"session", "reply", ...}, is how the turn ended.
//!
//! What the kernel promises, whatever goes wrong:
//!
//! - A problem with one process does not stop the others.
//! - A message is never dropped. It stays in the mailbox until a turn on it
//!   has finished, and the kernel keeps offering it until one does.
//! - At most one turn is at work in a process's directory, also after the
//!   kernel itself was killed and started again.
//! - When a message is given a second time, the recipient is told so.
//! - Ending a program ends everything it started.

use crate::children;
use crate::db::{self, Home, KERNEL, Proc, Res};
use crate::git;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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
    run: Option<Child>,
    /// Set when `.jarvis/run` has exited: the commit it was at. It is started
    /// again only once the process's code is at a different commit.
    run_down_at: Option<String>,
    /// A turn is in progress. `cancel` tells that turn to stop, and
    /// `turn_began_at` is the commit the directory was at when it began.
    busy: bool,
    cancel: Option<Arc<AtomicBool>>,
    turn_began_at: Option<String>,
    /// Turns that failed in a row, and when mail is offered next.
    failures: i64,
    retry_at: Option<Instant>,
    /// The last problem the kernel had with this process, so that a problem
    /// that persists is recorded once and not five times a second.
    trouble: Option<String>,
}

struct Mail {
    id: i64,
    from: String,
    body: String,
    /// 1 the first time it is given to the process, more after that.
    attempt: i64,
}

/// Everything a turn needs, so it can run on its own thread.
struct Job {
    id: String,
    label: String,
    dir: PathBuf,
    db_path: PathBuf,
    harness: PathBuf,
    session: Option<String>,
    env: Vec<(String, String)>,
    mail: Vec<Mail>,
    /// Something about the directory its agent should know (see `git::not_committable`).
    note: Option<String>,
    cancel: Arc<AtomicBool>,
}

enum Agent {
    /// The process's code handled every message.
    NotNeeded,
    Replied(Value),
    Failed(String),
    /// The kernel ended the turn (the process was stopped).
    Interrupted,
}

struct Done {
    id: String,
    by_code: Vec<i64>,
    for_agent: Vec<i64>,
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

/// Environment every program of a process gets: who it is, where jarvis is,
/// the `jarvis` command on its PATH, and its name on the commits it makes.
fn proc_env(home: &Home, proc: &Proc) -> Vec<(String, String)> {
    let mut path = std::env::var("PATH").unwrap_or_default();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        path = format!("{}:{path}", dir.display());
    }
    vec![
        ("JARVIS_HOME".into(), home.root.display().to_string()),
        ("JARVIS_ID".into(), proc.id.clone()),
        ("JARVIS_NAME".into(), proc.name.clone().unwrap_or_default()),
        ("JARVIS_GUIDE".into(), home.root.join("GUIDE.md").display().to_string()),
        ("GIT_AUTHOR_NAME".into(), proc.short().to_string()),
        ("GIT_COMMITTER_NAME".into(), proc.short().to_string()),
        ("PATH".into(), path),
    ]
}

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
        children::track(conn, pid, &job.id, kind);
    }
    // The turn may have been cancelled while this program was being started.
    if job.cancel.load(Ordering::SeqCst) {
        children::signal(pid, "KILL");
    }
    // A program that does not read its input is not an error.
    let _ = child.stdin.take().expect("stdin is piped").write_all(input.as_bytes());
    // Its errors are collected on the side, so that it can never be stuck
    // writing them while its output is being read here.
    let mut errors = child.stderr.take().expect("stderr is piped");
    let errors = std::thread::spawn(move || {
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
    children::signal(pid, "KILL");
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
    // What the agent does is recorded while it does it.
    let mut step = |line: &str| match serde_json::from_str::<Value>(line) {
        Ok(value) if value.get("step").is_some() => {
            if let Some(conn) = conn {
                let _ = db::event(conn, &job.id, "step", value);
            }
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
    let conn = db::open(&job.db_path).ok();
    let conn = conn.as_ref();
    let cancelled = || job.cancel.load(Ordering::SeqCst);
    let has_handle = is_executable(&job.dir.join(".jarvis/handle"));
    let (mut by_code, mut for_agent, mut prompt) = (Vec::new(), Vec::new(), String::new());
    for mail in &job.mail {
        let mut failure = None;
        if has_handle {
            match run_handle(&job, conn, mail) {
                Handled::Yes => {
                    by_code.push(mail.id);
                    continue;
                }
                Handled::NotMine => {}
                Handled::Failed(error) => failure = Some(error),
            }
        }
        if cancelled() {
            break;
        }
        for_agent.push(mail.id);
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
        let mut header = format!("[jarvis] You are process {}. {} new message(s).\n", job.label, for_agent.len());
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
    Done { id: job.id, by_code, for_agent, agent }
}

pub struct Kernel {
    home: Home,
    db: Connection,
    /// The id of this home. The kernel tends the processes that run here.
    here: String,
    live: HashMap<String, Live>,
    tx: mpsc::Sender<Done>,
    rx: mpsc::Receiver<Done>,
    last_event: i64,
}

impl Kernel {
    pub fn new(home: Home) -> Res<Kernel> {
        let db = home.open()?;
        let here = db::get_meta(&db, "home")?.ok_or("this repository is not a jarvis home yet (run `jarvis init`)")?;
        let (tx, rx) = mpsc::channel();
        let last_event = db.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))?;
        Ok(Kernel { home, db, here, live: HashMap::new(), tx, rx, last_event })
    }

    pub fn run(&mut self) -> Res<()> {
        self.take_over()?;
        ctrlc::set_handler(|| STOP.store(true, Ordering::SeqCst))?;
        eprintln!("jarvis is up at {} (home {}). Ctrl-C stops it.", self.home.root.display(), self.here);
        while !STOP.load(Ordering::SeqCst) {
            while let Ok(done) = self.rx.try_recv() {
                self.finish(done);
            }
            match db::procs(&self.db) {
                Ok(procs) => {
                    // A process that was removed is forgotten here too.
                    self.live.retain(|id, _| procs.iter().any(|p| &p.id == id));
                    for proc in &procs {
                        if proc.home != self.here {
                            continue;
                        }
                        // A problem with one process is that process's problem.
                        match self.tend(proc) {
                            Ok(()) => self.live.entry(proc.id.clone()).or_default().trouble = None,
                            Err(e) => self.trouble(&proc.id, &e.to_string()),
                        }
                    }
                }
                Err(e) => eprintln!("jarvis: could not read the registry: {e}"),
            }
            if let Err(e) = self.print_new_events() {
                eprintln!("jarvis: could not read the log: {e}");
            }
            thread::sleep(Duration::from_millis(200));
        }
        self.shut_down();
        Ok(())
    }

    /// Become the one kernel of this home, and clear up after a kernel that
    /// died: end the programs it left running, and put the mail it had handed
    /// out back in the mailboxes.
    fn take_over(&mut self) -> Res<()> {
        if let Some(other) = db::get_meta(&self.db, "kernel")? {
            if let Some((pid, since)) = other.split_once('\t') {
                if pid.parse::<u32>().is_ok_and(|pid| children::is_running(pid, since)) {
                    return Err(format!("a kernel is already running here (pid {pid})").into());
                }
            }
        }
        let pid = std::process::id();
        db::set_meta(&self.db, "kernel", &format!("{pid}\t{}", children::started(pid).unwrap_or_default()))?;

        let left = children::tracked(&self.db, None);
        if !left.is_empty() {
            eprintln!("jarvis: ending {} program(s) an earlier kernel left running", left.len());
            children::end_all(left);
        }
        self.db.execute("DELETE FROM children", [])?;
        self.db.execute("UPDATE messages SET status = 'pending' WHERE status = 'taken'", [])?;
        self.db.execute("UPDATE processes SET busy = 0, busy_since = NULL, failures = 0", [])?;
        fs::create_dir_all(self.home.dot().join("log"))?;
        Ok(())
    }

    /// Stop: end every program the kernel started. Mail that was being worked
    /// on goes back in the mailboxes and is given again next time.
    fn shut_down(&mut self) {
        eprintln!("jarvis: stopping");
        for live in self.live.values() {
            if let Some(cancel) = &live.cancel {
                cancel.store(true, Ordering::SeqCst);
            }
        }
        children::end_all(children::tracked(&self.db, None));
        for live in self.live.values_mut() {
            if let Some(mut child) = live.run.take() {
                let _ = child.wait();
            }
        }
        let _ = self.db.execute("DELETE FROM children", []);
        let _ = self.db.execute("UPDATE messages SET status = 'pending' WHERE status = 'taken'", []);
        let _ = self.db.execute("UPDATE processes SET busy = 0, busy_since = NULL", []);
        let _ = self.db.execute("DELETE FROM meta WHERE key = 'kernel'", []);
    }

    /// Record a problem the kernel had with one process, once.
    fn trouble(&mut self, id: &str, error: &str) {
        let live = self.live.entry(id.to_string()).or_default();
        if live.trouble.as_deref() == Some(error) {
            return;
        }
        live.trouble = Some(error.to_string());
        eprintln!("jarvis: problem with {id}: {error}");
        let _ = db::event(&self.db, id, "kernel_error", json!({"error": error}));
    }

    fn print_new_events(&mut self) -> Res<()> {
        for line in crate::event_lines(&self.db, self.last_event, None, false, &mut self.last_event)? {
            eprintln!("{line}");
        }
        Ok(())
    }

    /// Look after one process: its service, and its mail.
    fn tend(&mut self, proc: &Proc) -> Res<()> {
        let dir = self.home.proc_dir(&proc.id);
        if proc.status != "alive" {
            self.put_to_rest(proc);
            return Ok(());
        }
        self.live.entry(proc.id.clone()).or_default();
        // A service that cannot be started must not keep mail from the agent
        // who could fix it, so both are attempted.
        let service = self.tend_run(proc, &dir);
        let mail = self.dispatch(proc, &dir);
        service.and(mail)
    }

    /// The process was stopped: end its service and any turn in progress.
    fn put_to_rest(&mut self, proc: &Proc) {
        let Some(live) = self.live.get_mut(&proc.id) else { return };
        if let Some(mut child) = live.run.take() {
            children::end_child(&mut child);
            children::untrack(&self.db, child.id());
        }
        if let Some(cancel) = &live.cancel {
            if !cancel.swap(true, Ordering::SeqCst) {
                // Ended in the background, so the others are not kept waiting.
                let programs = children::tracked(&self.db, Some(&proc.id));
                thread::spawn(move || children::end_all(programs));
            }
        }
        // Being started again is a fresh start.
        if live.failures != 0 {
            live.failures = 0;
            let _ = self.db.execute("UPDATE processes SET failures = 0 WHERE id = ?1", [&proc.id]);
        }
        live.retry_at = None;
    }

    /// Keep `.jarvis/run` going: notice when it exits, start it when it should be up.
    fn tend_run(&mut self, proc: &Proc, dir: &Path) -> Res<()> {
        let log_path = self.home.dot().join("log").join(format!("{}.run.log", proc.id));
        let live = self.live.get_mut(&proc.id).expect("entry exists");

        if let Some(child) = &mut live.run {
            let Some(status) = child.try_wait()? else { return Ok(()) };
            let pid = child.id();
            live.run = None;
            live.run_down_at = Some(git::head(dir).unwrap_or_default());
            // Whatever it started goes with it.
            children::signal(pid, "KILL");
            children::untrack(&self.db, pid);
            let output = tail(&fs::read_to_string(&log_path).unwrap_or_default(), 40);
            db::event(&self.db, &proc.id, "run_exit", json!({"status": status.to_string(), "output": output}))?;
            if !status.success() {
                // The edge case goes to the agent.
                let notice = format!(
                    "[jarvis] Your .jarvis/run exited with {status}. It is started again once your code has changed.\n\nIts last output:\n{output}"
                );
                db::post(&self.db, KERNEL, &proc.id, &notice)?;
            }
            return Ok(());
        }

        let program = dir.join(".jarvis/run");
        if live.busy || !is_executable(&program) {
            return Ok(());
        }
        if let Some(commit) = &live.run_down_at {
            if git::head(dir).as_deref() == Some(commit.as_str()) {
                return Ok(());
            }
        }
        let log = File::create(&log_path)?;
        let child = Command::new(&program)
            .current_dir(dir)
            .envs(proc_env(&self.home, proc))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn();
        match child {
            Ok(child) => {
                children::track(&self.db, child.id(), &proc.id, "run");
                live.run = Some(child);
                live.run_down_at = None;
                db::event(&self.db, &proc.id, "run_start", json!({}))?;
            }
            Err(e) => {
                live.run_down_at = Some(git::head(dir).unwrap_or_default());
                db::event(&self.db, &proc.id, "run_exit", json!({"status": "could not start", "output": e.to_string()}))?;
                db::post(&self.db, KERNEL, &proc.id, &format!("[jarvis] Your .jarvis/run could not be started: {e}"))?;
            }
        }
        Ok(())
    }

    /// If the process is idle and has mail, start a turn for it.
    fn dispatch(&mut self, proc: &Proc, dir: &Path) -> Res<()> {
        let live = &self.live[&proc.id];
        if live.busy || live.retry_at.is_some_and(|at| Instant::now() < at) {
            return Ok(());
        }
        let waiting: Vec<(i64, String, String, i64)> = {
            let mut stmt = self.db.prepare_cached(
                "SELECT id, sender, body, attempts FROM messages WHERE recipient = ?1 AND status = 'pending' ORDER BY id",
            )?;
            stmt.query_map([&proc.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?
        };
        if waiting.is_empty() {
            return Ok(());
        }
        let mut mail = Vec::new();
        for (id, sender, body, attempts) in waiting {
            let from = match db::find_proc(&self.db, &sender)? {
                Some(sender) => sender.label(),
                None => sender,
            };
            mail.push(Mail { id, from, body, attempt: attempts + 1 });
        }
        let ids: Vec<i64> = mail.iter().map(|m| m.id).collect();

        // Hand the mail out: all of it, or none of it.
        let handing = self.db.unchecked_transaction()?;
        for id in &ids {
            handing.execute("UPDATE messages SET status = 'taken', attempts = attempts + 1 WHERE id = ?1", [id])?;
        }
        handing.execute("UPDATE processes SET busy = 1, busy_since = ?1 WHERE id = ?2", params![db::now_ms(), proc.id])?;
        handing.commit()?;
        db::event(&self.db, &proc.id, "turn_start", json!({"messages": ids}))?;

        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            id: proc.id.clone(),
            label: proc.label(),
            dir: dir.to_path_buf(),
            db_path: self.home.db_path(),
            harness: self.home.root.join("harness").join(&proc.harness),
            session: proc.session.clone(),
            env: proc_env(&self.home, proc),
            mail,
            note: git::not_committable(dir, &proc.branch),
            cancel: cancel.clone(),
        };
        let live = self.live.get_mut(&proc.id).expect("entry exists");
        live.busy = true;
        live.cancel = Some(cancel);
        live.turn_began_at = git::head(dir);
        let tx = self.tx.clone();
        let id = proc.id.clone();
        let started = thread::Builder::new().spawn(move || {
            let _ = tx.send(turn(job));
        });
        if let Err(e) = started {
            self.release(&id);
            return Err(format!("could not start a turn: {e}").into());
        }
        Ok(())
    }

    /// The process is no longer mid-turn: whatever mail is still marked as
    /// handed out goes back in its mailbox.
    fn release(&mut self, id: &str) {
        let requeued = self.db.execute("UPDATE messages SET status = 'pending' WHERE recipient = ?1 AND status = 'taken'", [id]);
        let freed = self.db.execute("UPDATE processes SET busy = 0, busy_since = NULL WHERE id = ?1", [id]);
        if let Some(live) = self.live.get_mut(id) {
            live.busy = false;
            live.cancel = None;
        }
        if let Err(e) = requeued.and(freed) {
            self.trouble(id, &format!("could not put its mail back: {e}"));
        }
    }

    /// A turn is over.
    fn finish(&mut self, done: Done) {
        let id = done.id.clone();
        if let Err(e) = self.record(done) {
            self.trouble(&id, &format!("could not record a finished turn: {e}"));
        }
        self.release(&id);
    }

    /// Record how a turn went, and commit what the process changed.
    fn record(&mut self, done: Done) -> Res<()> {
        let id = done.id.as_str();
        let proc = db::find_proc(&self.db, id)?.ok_or("the process is no longer in the registry")?;
        let handled = |ids: &[i64]| -> Res<()> {
            for message in ids {
                self.db.execute("UPDATE messages SET status = 'handled' WHERE id = ?1", [message])?;
            }
            Ok(())
        };
        handled(&done.by_code)?;
        if !done.by_code.is_empty() {
            db::event(&self.db, id, "handled_by_code", json!({"messages": done.by_code}))?;
        }

        let mut failures = 0;
        let subject: String = match done.agent {
            Agent::NotNeeded => "handled by code".into(),
            Agent::Replied(mut out) => {
                handled(&done.for_agent)?;
                if let Some(session) = out["session"].as_str() {
                    self.db.execute("UPDATE processes SET session = ?1 WHERE id = ?2", params![session, id])?;
                }
                let reply = out["reply"].as_str().unwrap_or("").to_string();
                // Everything the harness reported (cost, duration, ...) is kept as a fact.
                out["messages"] = json!(done.for_agent);
                db::event(&self.db, id, "turn", out)?;
                reply.lines().find(|l| !l.trim().is_empty()).unwrap_or("turn").chars().take(72).collect()
            }
            Agent::Interrupted => {
                db::event(&self.db, id, "turn_interrupted", json!({"messages": done.for_agent}))?;
                "turn interrupted".into()
            }
            Agent::Failed(error) => {
                // The mail stays in the mailbox and is offered again after a pause.
                failures = self.live.get(id).map_or(0, |l| l.failures) + 1;
                let delay = retry_delay(failures);
                db::event(
                    &self.db,
                    id,
                    "turn_failed",
                    json!({"messages": done.for_agent, "error": error, "failures": failures, "retry_in_s": delay.as_secs()}),
                )?;
                if let Some(live) = self.live.get_mut(id) {
                    live.retry_at = Some(Instant::now() + delay);
                }
                self.tell_senders(&proc, &done.for_agent, &error)?;
                "turn failed".into()
            }
        };
        if let Some(live) = self.live.get_mut(id) {
            live.failures = failures;
        }
        self.db.execute("UPDATE processes SET failures = ?1 WHERE id = ?2", params![failures, id])?;

        let dir = self.home.proc_dir(id);
        if let Some(why) = git::not_committable(&dir, &proc.branch) {
            db::event(&self.db, id, "commit_skipped", json!({"why": why}))?;
            return Ok(());
        }
        match git::commit_all(&dir, proc.short(), &format!("{}: {subject}", proc.short())) {
            Ok(Some(commit)) => db::event(&self.db, id, "commit", json!({"commit": commit, "subject": subject}))?,
            Ok(None) => {}
            Err(e) => db::event(&self.db, id, "commit_failed", json!({"error": e.to_string()}))?,
        }
        // Its code changed during the turn, whether the kernel committed it
        // just now or the agent did: a running service is stale, and a
        // stopped one gets another go.
        let Some(live) = self.live.get_mut(id) else { return Ok(()) };
        if git::head(&dir) != live.turn_began_at {
            if let Some(mut child) = live.run.take() {
                children::end_child(&mut child);
                children::untrack(&self.db, child.id());
            }
        }
        Ok(())
    }

    /// The first time a message could not be delivered, whoever sent it is told.
    fn tell_senders(&self, proc: &Proc, messages: &[i64], error: &str) -> Res<()> {
        for message in messages {
            let (sender, attempts): (String, i64) =
                self.db.query_row("SELECT sender, attempts FROM messages WHERE id = ?1", [message], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if attempts != 1 || sender == KERNEL || sender == proc.id {
                continue;
            }
            let notice = format!(
                "[jarvis] Your message {message} to {} has not been delivered yet: its agent could not run. \
                 The message stays in its mailbox and jarvis keeps trying; `jarvis ps` shows how it is going.\n\nThe error:\n{}",
                proc.label(),
                tail(error, 15),
            );
            db::post(&self.db, KERNEL, &sender, &notice)?;
        }
        Ok(())
    }
}
