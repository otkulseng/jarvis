//! `jarvis`: one command, used the same way by the human, by agents and by
//! process code. `jarvis up` is the kernel; everything else reads or writes
//! the registry and the git repository directly.
//!
//! The commands and their help are defined in `cli.rs`; this file does what
//! they say.

mod children;
mod cli;
mod db;
mod git;
mod kernel;

use clap::{CommandFactory, FromArgMatches};
use cli::{Cli, Cmd};
use db::{HUMAN, Home, KERNEL, Proc, Res};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::fs;
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn main() {
    // A reader that stops reading (`jarvis log | head`) ends this program
    // quietly, as it does any other command-line program.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    // `jarvis`, `jarvis help` and `jarvis --help` all print the grouped overview.
    let matches = Cli::command().override_help(cli::overview()).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let Some(command) = cli.command else {
        print!("{}", cli::overview());
        return;
    };
    if let Err(e) = run(command) {
        eprintln!("jarvis: {e}");
        std::process::exit(1);
    }
}

fn run(command: Cmd) -> Res<()> {
    match command {
        Cmd::Init => init(),
        Cmd::Up => kernel::Kernel::new(Home::find()?)?.run(),
        Cmd::Spawn(args) => spawn(args),
        Cmd::Ps(args) => ps(args),
        Cmd::Stop(args) => set_status(args, "stopped"),
        Cmd::Start(args) => set_status(args, "alive"),
        Cmd::Remove(args) => remove(args),
        Cmd::Tag(args) => tag(args),
        Cmd::Send(args) => send(args),
        Cmd::Messages(args) => messages(args),
        Cmd::Show(args) => show(args),
        Cmd::Log(args) => log(args),
        Cmd::Watch(args) => watch(args),
    }
}

// ---------------------------------------------------------------- helpers

/// Who is running this command: a process (the kernel sets `$JARVIS_ID` for
/// everything a process runs) or the human.
fn me() -> String {
    std::env::var("JARVIS_ID").ok().filter(|id| !id.is_empty()).unwrap_or_else(|| HUMAN.to_string())
}

/// Something worth knowing that is not an error. It goes to standard error,
/// so that a program reading the output is not confused by it.
fn note(text: &str) {
    eprintln!("note: {text}");
}

/// The pid of the kernel running in this home, if one is.
fn kernel_pid(conn: &Connection) -> Option<u32> {
    let value = db::get_meta(conn, "kernel").ok()??;
    let (pid, since) = value.split_once('\t')?;
    let pid: u32 = pid.parse().ok()?;
    children::is_running(pid, since).then_some(pid)
}

const NO_KERNEL: &str = "the kernel is not running here, so nothing happens until `jarvis up` is started";

/// The name of whoever has this address: a process's name, `human`, `kernel`.
fn label(conn: &Connection, address: &str) -> String {
    match db::find_proc(conn, address) {
        Ok(Some(proc)) => proc.short().to_string(),
        _ => address.to_string(),
    }
}

fn find(conn: &Connection, who: &str) -> Res<Proc> {
    db::find_proc(conn, who)?.ok_or_else(|| format!("there is no process called `{who}` (`jarvis ps` lists them)").into())
}

/// The first line of real text, shortened.
fn clip(text: &str, limit: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() > limit {
        format!("{}…", line.chars().take(limit).collect::<String>())
    } else {
        line.to_string()
    }
}

/// `2m`, `40s`: how long ago a moment (in milliseconds) was.
fn ago(ms: i64) -> String {
    let seconds = (db::now_ms() - ms).max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        _ => format!("{}h{}m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// The first line of real text in a process's `.jarvis/ABOUT.md`: how it
/// describes itself.
fn about(dir: &Path) -> String {
    fs::read_to_string(dir.join(".jarvis/ABOUT.md"))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("")
        .to_string()
}

/// Why a message is not dealt with yet, in words; empty if it is.
fn waiting(status: &str, attempts: i64) -> String {
    match (status, attempts) {
        ("handled", _) => String::new(),
        ("taken", 0 | 1) => "being worked on".into(),
        ("taken", n) => format!("being worked on, attempt {n}"),
        (_, 0) => "waiting".into(),
        (_, 1) => "waiting; one turn on it did not finish".into(),
        (_, n) => format!("waiting; {n} turns on it did not finish"),
    }
}

// --------------------------------------------------------------- commands

fn init() -> Res<()> {
    let here = std::env::current_dir()?;
    let root = git::git(&here, &["rev-parse", "--show-toplevel"])
        .map_err(|_| "jarvis lives in a git repository, and this directory is not in one. Run `git init` first.")?;
    let home = Home { root: root.into() };
    fs::create_dir_all(home.dot())?;
    let conn = home.open()?;
    // A home names itself. The id only tells homes apart once there are
    // several; nobody has to choose or remember it.
    let id = match db::get_meta(&conn, "home")? {
        Some(existing) => existing,
        None => {
            let id = db::random_hex(4)?;
            db::set_meta(&conn, "home", &id)?;
            id
        }
    };
    println!("jarvis home at {} (home {id})", home.root.display());
    println!("next: `jarvis up` in one terminal, then `jarvis spawn NAME ...` in another (see `jarvis help spawn`)");
    Ok(())
}

fn spawn(args: cli::Spawn) -> Res<()> {
    let name = args.name.as_str();
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    if name.is_empty() || !name.chars().all(plain) || name.starts_with(['-', '.']) {
        return Err(format!("`{name}` cannot be a name: use letters, digits, `-`, `_` and `.`, starting with a letter or digit").into());
    }
    let home = Home::find()?;
    let conn = home.open()?;
    let here = db::get_meta(&conn, "home")?.ok_or("this repository is not a jarvis home yet: run `jarvis init`")?;
    if name == HUMAN || name == KERNEL {
        return Err(format!("`{name}` is reserved; choose another name").into());
    }
    if let Some(existing) = db::find_proc(&conn, name)? {
        return Err(format!(
            "there already is a process called `{name}` (made by {}); choose another name",
            label(&conn, &existing.parent)
        )
        .into());
    }
    // The name is the process: its address, its directory and its branch.
    let id = name.to_string();
    let branch = format!("jarvis/{name}");
    if git::git(&home.root, &["check-ref-format", "--branch", &branch]).is_err() {
        return Err(format!("`{name}` cannot be a name: git does not accept `{branch}` as a branch name").into());
    }
    if git::git(&home.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok() {
        return Err(format!(
            "there already is a branch `{branch}`, though no process is called `{name}` (it was kept when a process \
             was removed, or made by hand). Choose another name, or delete that branch first if nothing needs it"
        )
        .into());
    }

    let prompt = match (&args.prompt, &args.prompt_file) {
        (Some(text), _) => Some(text.clone()),
        (None, Some(file)) => Some(fs::read_to_string(file).map_err(|e| format!("could not read {file}: {e}"))?),
        (None, None) => None,
    };

    // Where it starts from: anything git knows (an expert's branch, a
    // process's branch `jarvis/NAME`, a tag, a commit), then a process by
    // name, then a branch that so far exists only on the remote.
    let mut forked: Option<Proc> = None;
    let start = match &args.from {
        None => None,
        Some(what) => {
            let commit_of = |what: &str| git::git(&home.root, &["rev-parse", "--verify", "--quiet", &format!("{what}^{{commit}}")]).ok();
            if let Some(commit) = commit_of(what) {
                if let Some(of) = what.strip_prefix("jarvis/") {
                    forked = db::find_proc(&conn, of)?;
                }
                Some(commit)
            } else if let Some(proc) = db::find_proc(&conn, what)? {
                let branch = proc.branch.clone();
                forked = Some(proc);
                Some(branch)
            } else if let Some(commit) = commit_of(&format!("origin/{what}")) {
                Some(commit)
            } else {
                return Err(format!(
                    "`{what}` is neither a branch, tag or commit that git knows, nor a process (`jarvis ps` lists them)"
                )
                .into());
            }
        }
    };
    if let Some(tree) = &args.tree {
        let tree = Path::new(tree);
        if !tree.is_dir() {
            return Err(format!("{} is not a directory", tree.display()).into());
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
    let blank = prompt.is_none() && start.is_none() && args.tree.is_none();
    let tags: Vec<(String, String)> = args.tags.iter().map(|t| parse_tag(t)).collect::<Res<_>>()?;

    let dir = home.proc_dir(&id);
    fs::create_dir_all(home.dot().join("proc"))?;
    let parent = me();
    match &start {
        Some(start) => git::worktree_fork(&home.root, &dir, &branch, start)?,
        None => git::worktree_new(&home.root, &dir, &branch)?,
    }

    // From here on a failure must not leave half a process behind.
    let fill = || -> Res<()> {
        if let Some(tree) = &args.tree {
            let source = format!("{}/.", tree.trim_end_matches('/'));
            let copied = Command::new("cp").args(["-R", &source, &dir.to_string_lossy()]).status()?;
            if !copied.success() {
                return Err(format!("could not copy {tree}").into());
            }
        }
        if let Some(prompt) = &prompt {
            fs::create_dir_all(dir.join(".jarvis"))?;
            fs::write(dir.join(".jarvis/PROMPT.md"), format!("{}\n", prompt.trim_end()))?;
        }
        let message = format!("{name}: spawned by {}", label(&conn, &parent));
        if git::commit_all(&dir, name, &message)?.is_none() {
            git::commit_empty(&dir, name, &message)?;
        }
        conn.execute(
            "INSERT INTO processes (id, name, home, branch, harness, status, parent, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'alive', ?6, ?7)",
            params![id, name, here, branch, args.harness, parent, db::now_ms()],
        )?;
        set_tags(&conn, &id, &tags)?;
        db::event(&conn, &id, "spawn", json!({"name": name, "parent": parent, "from": args.from, "tree": args.tree}))?;
        Ok(())
    };
    if let Err(e) = fill() {
        let _ = git::git(&home.root, &["worktree", "remove", "--force", &dir.to_string_lossy()]);
        let _ = git::git(&home.root, &["branch", "-D", &branch]);
        return Err(format!("could not create `{name}`: {e}").into());
    }

    // A first message, from whoever made it, wakes it up. A blank process
    // gets none: it waits for someone to tell it what to be.
    if !blank {
        let maker = label(&conn, &parent);
        let has_prompt = dir.join(".jarvis/PROMPT.md").exists();
        // Said when it starts from a branch that is not a process: an expert.
        let is_branch = |what: &str| {
            ["refs/heads", "refs/remotes/origin"]
                .iter()
                .any(|place| git::git(&home.root, &["rev-parse", "--verify", "--quiet", &format!("{place}/{what}")]).is_ok())
        };
        let began = match &args.from {
            Some(what) if is_branch(what) => format!(
                " Your directory began as a copy of the branch `{what}`: that is your expert, and where your finished \
                 work goes back to (the guide says how)."
            ),
            Some(what) => format!(" Your directory began as a copy of `{what}`."),
            None => String::new(),
        };
        let hello = match (&forked, &prompt, has_prompt) {
            (Some(origin), Some(_), _) => format!(
                "[jarvis] You have just been created by {maker} as a fork of {}: your directory began as a copy of theirs, \
                 notes included, but you are a new process called `{name}` with a memory of your own. \
                 Your instructions are in .jarvis/PROMPT.md. Read them and begin.",
                origin.label()
            ),
            (Some(origin), None, _) => format!(
                "[jarvis] You have just been created by {maker} as a fork of {}: your directory began as a copy of theirs, \
                 notes and prompt included, but you are a new process called `{name}` with a memory of your own. \
                 You were given no instructions of your own. Look at what is in your directory, and ask {maker} what you are for.",
                origin.label()
            ),
            (None, _, true) => format!(
                "[jarvis] You have just been created by {maker}.{began} Your instructions are in .jarvis/PROMPT.md. Read them and begin."
            ),
            (None, _, false) => format!("[jarvis] You have just been created by {maker}.{began} Look at what is in your directory and begin."),
        };
        db::post(&conn, &parent, &id, &hello)?;
    }
    println!("{id}");
    if !is_harness(&home, &args.harness) {
        note(&format!("there is no harness adapter `harness/{}` in this home, so its agent cannot run until there is", args.harness));
    }
    if blank {
        note(&format!("`{name}` is blank: it does nothing until it gets a message (`jarvis send {name} ...`)"));
    }
    if kernel_pid(&conn).is_none() {
        note(NO_KERNEL);
    }
    Ok(())
}

fn is_harness(home: &Home, name: &str) -> bool {
    kernel::is_executable(&home.root.join("harness").join(name))
}

/// Everything `ps` knows about one process.
fn describe(home: &Home, conn: &Connection, p: &Proc) -> Res<Value> {
    let dir = home.proc_dir(&p.id);
    // What its agent has cost so far, as far as its harness reported.
    let (turns, cost): (i64, Option<f64>) = conn.query_row(
        "SELECT COUNT(*), SUM(json_extract(data, '$.cost_usd')) FROM events WHERE proc = ?1 AND kind = 'turn'",
        [&p.id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mail: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages WHERE recipient = ?1 AND status != 'handled'", [&p.id], |r| r.get(0))?;
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
    // What it was started from (an expert's branch, another process), if anything.
    let from: Option<String> = conn
        .query_row(
            "SELECT json_extract(data, '$.from') FROM events WHERE proc = ?1 AND kind = 'spawn' ORDER BY id DESC LIMIT 1",
            [&p.id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(json!({
        "id": p.id, "name": p.name, "state": state, "about": about(&dir), "from": from, "tags": tags_of(conn, &p.id)?,
        "home": p.home, "branch": p.branch, "directory": dir.display().to_string(), "harness": p.harness,
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

fn ps(args: cli::Ps) -> Res<()> {
    let home = Home::find()?;
    let conn = home.open()?;
    if kernel_pid(&conn).is_none() {
        note(NO_KERNEL);
    }
    if let Some(who) = &args.who {
        return ps_one(&home, &conn, &find(&conn, who)?, args.json);
    }
    let rows: Vec<Value> = db::procs(&conn)?.iter().map(|p| describe(&home, &conn, p)).collect::<Res<_>>()?;
    if args.json {
        println!("{}", Value::Array(rows));
        return Ok(());
    }
    if rows.is_empty() {
        println!("no processes yet. `jarvis help spawn` shows how to create one.");
        return Ok(());
    }
    println!("{:<16} {:<13} {:<12} ABOUT", "NAME", "STATE", "MADE BY");
    for r in rows {
        println!(
            "{:<16} {:<13} {:<12} {}",
            r["name"].as_str().unwrap_or("-"),
            state_words(&r),
            label(&conn, r["parent"].as_str().unwrap_or("")),
            r["about"].as_str().unwrap_or(""),
        );
    }
    Ok(())
}

/// Everything about one process.
fn ps_one(home: &Home, conn: &Connection, p: &Proc, as_json: bool) -> Res<()> {
    let mut row = describe(home, conn, p)?;
    let service_pid: Option<u32> = conn
        .query_row("SELECT pid, started FROM children WHERE proc = ?1 AND kind = 'run'", [&p.id], |r| {
            Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))
        })
        .ok()
        .filter(|(pid, since)| children::is_running(*pid, since))
        .map(|(pid, _)| pid);
    let last_of = |kinds: &str| -> Option<(i64, String, Value)> {
        conn.query_row(
            &format!("SELECT at, kind, data FROM events WHERE proc = ?1 AND kind IN ({kinds}) ORDER BY id DESC LIMIT 1"),
            [&p.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?)),
        )
        .ok()
        .map(|(at, kind, data)| (at, kind, serde_json::from_str(&data).unwrap_or(Value::Null)))
    };
    let last_turn = last_of("'turn', 'turn_failed', 'turn_interrupted'");
    let last_exit = last_of("'run_exit'");
    let head = git::git(&home.root, &["rev-parse", "--short", &p.branch]).ok();
    row["commit"] = json!(head);
    row["service_pid"] = json!(service_pid);
    row["last_turn"] = match &last_turn {
        Some((at, kind, data)) => json!({"at": at, "kind": kind, "reply": data["reply"], "error": data["error"], "cost_usd": data["cost_usd"]}),
        None => Value::Null,
    };
    if as_json {
        println!("{row}");
        return Ok(());
    }

    let line = |key: &str, value: String| println!("{key:<10} {value}");
    line("name", p.short().to_string());
    line("state", state_words(&row));
    line("about", match row["about"].as_str() {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => "(has not described itself yet)".into(),
    });
    line("made by", format!("{}, {} ago", label(conn, &p.parent), ago(p.created_at)));
    line("harness", p.harness.clone());
    if let Some(tags) = row["tags"].as_object().filter(|tags| !tags.is_empty()) {
        line("tags", tags.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join(", "));
    }
    line("branch", format!("{}{}", p.branch, head.map(|h| format!(" (at {h})")).unwrap_or_default()));
    line("directory", home.proc_dir(&p.id).display().to_string());
    line("service", match (row["has_run"].as_bool(), service_pid, &last_exit) {
        (Some(true), Some(pid), _) => format!("running (pid {pid})"),
        (Some(true), None, Some((at, _, data))) => {
            format!("not running; it exited {} ago with {}", ago(*at), data["status"].as_str().unwrap_or("?"))
        }
        (Some(true), None, None) => "not running".into(),
        _ => "none (no .jarvis/run)".into(),
    });
    line("handler", if row["has_handle"] == true { "yes (.jarvis/handle)".into() } else { "none".into() });
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
                    println!("{:<10} {text}", "");
                }
            }
        },
    }
    Ok(())
}

fn set_status(args: cli::Who, status: &str) -> Res<()> {
    let conn = Home::find()?.open()?;
    // All of them must exist before any is touched.
    let procs: Vec<Proc> = args.who.iter().map(|who| find(&conn, who)).collect::<Res<_>>()?;
    for proc in procs {
        if proc.status == status {
            println!("{} was already {status}", proc.label());
            continue;
        }
        conn.execute("UPDATE processes SET status = ?1 WHERE id = ?2", params![status, proc.id])?;
        db::event(&conn, &proc.id, if status == "alive" { "start" } else { "stop" }, json!({"by": me()}))?;
        println!("{} is {status}", proc.label());
    }
    if kernel_pid(&conn).is_none() {
        note("the kernel is not running here; this takes effect when `jarvis up` is started");
    }
    Ok(())
}

/// `NAME=VALUE` as a tag. An empty value means "remove this tag".
fn parse_tag(text: &str) -> Res<(String, String)> {
    let (key, value) = text.split_once('=').ok_or_else(|| format!("`{text}` is not a tag: write NAME=VALUE, such as project=moe"))?;
    let key = key.trim();
    let plain = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.');
    if key.is_empty() || !key.chars().all(plain) {
        return Err(format!("`{key}` cannot be the name of a tag: use letters, digits, spaces, `-`, `_` and `.`").into());
    }
    Ok((key.to_string(), value.trim().to_string()))
}

fn set_tags(conn: &Connection, proc: &str, tags: &[(String, String)]) -> Res<()> {
    for (key, value) in tags {
        if value.is_empty() {
            conn.execute("DELETE FROM tags WHERE proc = ?1 AND key = ?2", params![proc, key])?;
        } else {
            conn.execute(
                "INSERT INTO tags (proc, key, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT(proc, key) DO UPDATE SET value = excluded.value",
                params![proc, key, value],
            )?;
        }
    }
    Ok(())
}

/// The tags of a process, as a JSON object.
fn tags_of(conn: &Connection, proc: &str) -> Res<Value> {
    let mut stmt = conn.prepare_cached("SELECT key, value FROM tags WHERE proc = ?1 ORDER BY key")?;
    let tags: serde_json::Map<String, Value> =
        stmt.query_map([proc], |r| Ok((r.get::<_, String>(0)?, Value::String(r.get(1)?))))?.collect::<Result<_, _>>()?;
    Ok(Value::Object(tags))
}

fn tag(args: cli::Tag) -> Res<()> {
    let conn = Home::find()?.open()?;
    let proc = find(&conn, &args.who)?;
    let tags: Vec<(String, String)> = args.tags.iter().map(|t| parse_tag(t)).collect::<Res<_>>()?;
    if !tags.is_empty() {
        set_tags(&conn, &proc.id, &tags)?;
        let set: serde_json::Map<String, Value> = tags.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        db::event(&conn, &proc.id, "tag", json!({"by": me(), "set": set}))?;
    }
    match tags_of(&conn, &proc.id)?.as_object() {
        Some(now) if !now.is_empty() => {
            for (key, value) in now {
                println!("{key}={}", value.as_str().unwrap_or(""));
            }
        }
        _ => println!("{} has no tags", proc.short()),
    }
    Ok(())
}

/// Delete processes for good.
fn remove(args: cli::Remove) -> Res<()> {
    let home = Home::find()?;
    let conn = home.open()?;
    // All of them must exist before any is touched.
    let procs: Vec<Proc> = args.who.iter().map(|who| find(&conn, who)).collect::<Res<_>>()?;
    let by = me();
    if procs.iter().any(|p| p.id == by) {
        return Err("a process cannot remove itself: this command is one of the programs that would be ended. Ask another process, or the human".into());
    }
    for proc in &procs {
        conn.execute("UPDATE processes SET status = 'stopped' WHERE id = ?1", [&proc.id])?;
    }
    for proc in procs {
        // Nothing of it may still be running when its directory goes.
        if kernel_pid(&conn).is_some() {
            // The kernel ends its service and turn, as for `jarvis stop`. Give
            // it time to see the change, then wait until it has let go.
            std::thread::sleep(Duration::from_millis(500));
            let let_go = || -> Res<bool> {
                let busy: bool = conn.query_row("SELECT busy FROM processes WHERE id = ?1", [&proc.id], |r| r.get(0))?;
                Ok(!busy && children::tracked(&conn, Some(&proc.id)).is_empty())
            };
            let mut waited = 0;
            while !let_go()? {
                if waited >= 150 {
                    return Err(format!(
                        "{} is stopped but the kernel has not finished ending what it was doing, so nothing of it was deleted. Try again",
                        proc.label()
                    )
                    .into());
                }
                std::thread::sleep(Duration::from_millis(100));
                waited += 1;
            }
        } else {
            children::end_all(children::tracked(&conn, Some(&proc.id)));
        }

        let dir = home.proc_dir(&proc.id);
        let commit = git::git(&home.root, &["rev-parse", "--short", &proc.branch]).ok();
        let (turns, cost): (i64, Option<f64>) = conn.query_row(
            "SELECT COUNT(*), SUM(json_extract(data, '$.cost_usd')) FROM events WHERE proc = ?1 AND kind = 'turn'",
            [&proc.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if dir.exists() {
            git::git(&home.root, &["worktree", "remove", "--force", &dir.to_string_lossy()])
                .map_err(|e| format!("could not delete the directory of {}, so nothing of it was deleted: {e}", proc.label()))?;
        }
        let _ = git::git(&home.root, &["worktree", "prune"]);
        if !args.keep_branch && commit.is_some() {
            git::git(&home.root, &["branch", "-D", &proc.branch])?;
        }
        let _ = fs::remove_file(home.dot().join("log").join(format!("{}.run.log", proc.id)));
        conn.execute_batch("BEGIN")?;
        let forget = || -> Res<()> {
            conn.execute("DELETE FROM children WHERE proc = ?1", [&proc.id])?;
            conn.execute("DELETE FROM tags WHERE proc = ?1", [&proc.id])?;
            conn.execute("DELETE FROM messages WHERE sender = ?1 OR recipient = ?1", [&proc.id])?;
            conn.execute("DELETE FROM events WHERE proc = ?1 OR (kind = 'send' AND json_extract(data, '$.to') = ?1)", [&proc.id])?;
            conn.execute("DELETE FROM processes WHERE id = ?1", [&proc.id])?;
            db::event(&conn, &by, "remove", json!({
                "name": proc.id, "commit": commit, "branch_kept": args.keep_branch, "turns": turns, "cost_usd": cost,
            }))?;
            Ok(())
        };
        match forget() {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(e);
            }
        }
        match (&commit, args.keep_branch) {
            (Some(commit), true) => println!("removed {}; its branch `{}` stays, at {commit}", proc.id, proc.branch),
            (Some(commit), false) => println!(
                "removed {} (its branch was at {commit}; `git branch {} {commit}` brings its files back while git still has them)",
                proc.id, proc.branch
            ),
            (None, _) => println!("removed {}", proc.id),
        }
    }
    Ok(())
}

fn send(args: cli::SendArgs) -> Res<()> {
    let mut body = args.text.join(" ");
    if body == "-" || body.is_empty() {
        if std::io::stdin().is_terminal() {
            eprintln!("type the message; finish with Ctrl-D on a line of its own");
        }
        body.clear();
        std::io::stdin().read_to_string(&mut body)?;
    }
    if body.trim().is_empty() {
        return Err("there was nothing to send: the message was empty".into());
    }
    let conn = Home::find()?.open()?;
    let sender = match &args.sender {
        Some(who) => db::resolve(&conn, who)?,
        None => me(),
    };
    let to = db::resolve(&conn, &args.to)?;
    let id = db::post(&conn, &sender, &to, body.trim_end())?;
    println!("sent message {id} to {}", label(&conn, &to));

    // If it will not be read soon, say why.
    if let Some(proc) = db::find_proc(&conn, &to)? {
        if proc.status != "alive" {
            note(&format!("{} is stopped; the message waits until `jarvis start {}`", proc.short(), proc.short()));
        } else if kernel_pid(&conn).is_none() {
            note(NO_KERNEL);
        } else if proc.failures > 0 {
            note(&format!(
                "{}'s last {} turn(s) failed; the message waits while the kernel keeps trying (`jarvis ps {}` shows why)",
                proc.short(),
                proc.failures,
                proc.short()
            ));
        }
    }
    Ok(())
}

fn messages(args: cli::Messages) -> Res<()> {
    let conn = Home::find()?.open()?;
    if args.pairs {
        return pairs(&conn, args.json);
    }
    let with = match &args.who {
        Some(who) => Some(db::resolve(&conn, who)?),
        None => None,
    };
    let other = match &args.with {
        Some(who) => Some(db::resolve(&conn, who)?),
        None => None,
    };
    let count = args.count.unwrap_or(if args.json { 500 } else { 20 });
    let mut stmt = conn.prepare(
        "SELECT id, created_at, sender, recipient, body, status, attempts,
                strftime('%H:%M:%S', created_at / 1000, 'unixepoch', 'localtime')
         FROM messages
         WHERE id > ?1 AND (?2 IS NULL OR sender = ?2 OR recipient = ?2) AND (?3 = 0 OR status != 'handled')
           AND (?5 IS NULL OR sender = ?5 OR recipient = ?5) AND (?6 IS NULL OR id < ?6)
         ORDER BY id DESC LIMIT ?4",
    )?;
    let mut rows: Vec<(Value, String)> = stmt
        .query_map(params![args.after, with, args.waiting, count, other, args.before], |r| {
            Ok((
                json!({
                    "id": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "from": r.get::<_, String>(2)?,
                    "to": r.get::<_, String>(3)?, "body": r.get::<_, String>(4)?, "status": r.get::<_, String>(5)?,
                    "attempts": r.get::<_, i64>(6)?,
                }),
                r.get::<_, String>(7)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    rows.reverse();
    if args.json {
        println!("{}", Value::Array(rows.into_iter().map(|(row, _)| row).collect()));
        return Ok(());
    }
    if rows.is_empty() {
        println!("no messages{}", if args.waiting { " are waiting" } else { "" });
    }
    for (m, time) in rows {
        let state = waiting(m["status"].as_str().unwrap_or(""), m["attempts"].as_i64().unwrap_or(0));
        println!(
            "#{} {time} {} → {}{}\n{}\n",
            m["id"],
            label(&conn, m["from"].as_str().unwrap_or("")),
            label(&conn, m["to"].as_str().unwrap_or("")),
            if state.is_empty() { String::new() } else { format!("  ({state})") },
            m["body"].as_str().unwrap_or(""),
        );
    }
    Ok(())
}

/// Who has written to whom, and how much.
fn pairs(conn: &Connection, as_json: bool) -> Res<()> {
    let mut stmt = conn.prepare(
        "SELECT MIN(sender, recipient), MAX(sender, recipient), COUNT(*), SUM(status != 'handled'),
                SUM(body LIKE '[jarvis]%'), MAX(created_at), MAX(id)
         FROM messages WHERE sender != recipient GROUP BY 1, 2 ORDER BY 6",
    )?;
    let rows: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "a": r.get::<_, String>(0)?, "b": r.get::<_, String>(1)?, "count": r.get::<_, i64>(2)?,
                "waiting": r.get::<_, i64>(3)?, "by_jarvis": r.get::<_, i64>(4)?, "last_at": r.get::<_, i64>(5)?,
                "last_id": r.get::<_, i64>(6)?,
            }))
        })?
        .collect::<Result<_, _>>()?;
    if as_json {
        println!("{}", Value::Array(rows));
        return Ok(());
    }
    if rows.is_empty() {
        println!("nobody has written to anybody yet");
    }
    for r in rows {
        let waiting = match r["waiting"].as_i64().unwrap_or(0) {
            0 => String::new(),
            n => format!(", {n} waiting"),
        };
        println!(
            "{} and {}: {} message(s){waiting}",
            label(conn, r["a"].as_str().unwrap_or("")),
            label(conn, r["b"].as_str().unwrap_or("")),
            r["count"]
        );
    }
    Ok(())
}

fn message_json(conn: &Connection, id: i64) -> Option<Value> {
    conn.query_row(
        "SELECT id, created_at, sender, recipient, body, status, attempts FROM messages WHERE id = ?1",
        [id],
        |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "from": r.get::<_, String>(2)?,
                "to": r.get::<_, String>(3)?, "body": r.get::<_, String>(4)?, "status": r.get::<_, String>(5)?,
                "attempts": r.get::<_, i64>(6)?,
            }))
        },
    )
    .ok()
}

/// Print every change as a JSON line, for as long as someone is reading.
fn watch(args: cli::Watch) -> Res<()> {
    use std::collections::{BTreeSet, HashMap};
    use std::io::Write;
    let home = Home::find()?;
    let conn = home.open()?;
    let out = std::io::stdout();
    // A reader that went away ends this quietly.
    let say = |line: Value| -> bool {
        let mut out = out.lock();
        writeln!(out, "{line}").is_ok()
    };

    let mut cursor: i64 = match args.after {
        Some(after) => after,
        None => conn.query_row("SELECT COALESCE(MAX(id), 0) FROM events", [], |r| r.get(0))?,
    };
    // What was last said about each process, and its registry row then.
    let mut said: HashMap<String, (String, String)> = HashMap::new();
    // The messages not dealt with yet, and their status when last seen.
    let mut open: HashMap<i64, (String, i64)> = HashMap::new();
    let mut version: i64 = -1;
    let mut first = true;

    loop {
        // Has any `jarvis` command or the kernel written anything since the last look?
        let now: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
        if now != version {
            version = now;
            let mut touched: BTreeSet<String> = BTreeSet::new();

            // New facts, in order. (The first look only says how things stand.)
            while !first {
                let mut stmt = conn.prepare_cached(
                    "SELECT id, at, proc, kind, data FROM events WHERE id > ?1 AND (?2 OR kind != 'step') ORDER BY id LIMIT 2000",
                )?;
                let facts: Vec<(i64, i64, String, String, String)> = stmt
                    .query_map(params![cursor, args.steps], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                    .collect::<Result<_, _>>()?;
                let more = facts.len() == 2000;
                for (id, at, proc, kind, data) in facts {
                    cursor = id;
                    let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                    let mut line = json!({"kind": "fact", "fact": {"id": id, "at": at, "proc": proc, "kind": kind, "data": data}});
                    if kind == "send" {
                        if let Some(to) = data["to"].as_str() {
                            touched.insert(to.to_string());
                        }
                        line["message"] = data["message"].as_i64().and_then(|m| message_json(&conn, m)).unwrap_or(Value::Null);
                    }
                    if kind != "step" {
                        touched.insert(proc);
                    }
                    if !say(line) {
                        return Ok(());
                    }
                }
                if !more {
                    break;
                }
            }

            // Messages whose status changed.
            let mut stmt = conn.prepare_cached("SELECT id, status, attempts FROM messages WHERE status != 'handled'")?;
            let waiting: HashMap<i64, (String, i64)> =
                stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?.collect::<Result<_, _>>()?;
            let mut changed: Vec<i64> = waiting.iter().filter(|(id, state)| open.get(*id) != Some(*state)).map(|(id, _)| *id).collect();
            changed.extend(open.keys().filter(|id| !waiting.contains_key(*id)));
            changed.sort();
            open = waiting;
            if !first {
                for id in changed {
                    if let Some(message) = message_json(&conn, id) {
                        for who in ["from", "to"] {
                            touched.insert(message[who].as_str().unwrap_or("").to_string());
                        }
                        if !say(json!({"kind": "message", "message": message})) {
                            return Ok(());
                        }
                    }
                }
            }

            // Processes: those something happened to, and those whose registry row changed.
            let procs = db::procs(&conn)?;
            for p in &procs {
                let row = format!("{}|{}|{:?}|{}|{:?}", p.status, p.busy, p.busy_since, p.failures, p.session);
                let known = said.get(&p.id);
                if known.is_some_and(|(_, was)| *was == row) && !touched.contains(&p.id) {
                    continue;
                }
                let described = describe(&home, &conn, p)?.to_string();
                if known.is_some_and(|(was, _)| *was == described) {
                    said.insert(p.id.clone(), (described, row));
                    continue;
                }
                let line = format!("{{\"kind\":\"process\",\"process\":{described}}}");
                said.insert(p.id.clone(), (described, row));
                if writeln!(out.lock(), "{line}").is_err() {
                    return Ok(());
                }
            }
            let gone: Vec<String> = said.keys().filter(|name| !procs.iter().any(|p| &p.id == *name)).cloned().collect();
            for name in gone {
                said.remove(&name);
                if !say(json!({"kind": "gone", "name": name})) {
                    return Ok(());
                }
            }
            if first {
                first = false;
                // The facts above the starting point, if any, are next.
                version = -1;
                if !say(json!({"kind": "ready", "cursor": cursor})) {
                    return Ok(());
                }
            }
            if out.lock().flush().is_err() {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn show(args: cli::Show) -> Res<()> {
    let home = Home::find()?;
    let proc = find(&home.open()?, &args.who)?;
    let out = match &args.path {
        None => git::git(&home.root, &["ls-tree", "-r", "--name-only", &proc.branch])?,
        Some(path) => git::git(&home.root, &["show", &format!("{}:{path}", proc.branch)]).map_err(|_| {
            format!("{} has no file `{path}` (`jarvis show {}` lists its files)", proc.short(), proc.short())
        })?,
    };
    println!("{out}");
    Ok(())
}

/// Facts are about one process when it did them, or when they are a message to it.
const ABOUT_PROC: &str = "(?2 IS NULL OR proc = ?2 OR (kind = 'send' AND json_extract(data, '$.to') = ?2))";
/// A step is something an agent did inside a turn. There are many, so they
/// are shown only when asked for.
const STEPS: &str = "(?3 OR kind != 'step')";

/// Facts after `after` (only those about `who`, if given), as lines for a
/// terminal. `last` is moved to the newest fact seen.
pub fn event_lines(conn: &Connection, after: i64, who: Option<&str>, steps: bool, last: &mut i64) -> Res<Vec<String>> {
    event_lines_before(conn, after, None, who, steps, last)
}

fn event_lines_before(conn: &Connection, after: i64, before: Option<i64>, who: Option<&str>, steps: bool, last: &mut i64) -> Res<Vec<String>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT id, strftime('%H:%M:%S', at / 1000, 'unixepoch', 'localtime'), proc, kind, data
         FROM events WHERE id > ?1 AND {ABOUT_PROC} AND {STEPS} AND (?4 IS NULL OR id < ?4) ORDER BY id"
    ))?;
    let rows: Vec<(i64, String, String, String, String)> = stmt
        .query_map(params![after, who, steps, before], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    let mut lines = Vec::new();
    for (id, time, proc, kind, data) in rows {
        *last = id;
        let d: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
        let text = |key: &str| d[key].as_str().unwrap_or("").to_string();
        let what = match kind.as_str() {
            "send" => {
                let body: String = conn
                    .query_row("SELECT body FROM messages WHERE id = ?1", [d["message"].as_i64()], |r| r.get(0))
                    .unwrap_or_default();
                format!("→ {}: {}", label(conn, &text("to")), clip(&body, 140))
            }
            "turn" => {
                let cost = d["cost_usd"].as_f64().map(|c| format!(" (${c:.3})")).unwrap_or_default();
                format!("turn{cost}: {}", clip(&text("reply"), 140))
            }
            "turn_start" => format!("turn started on {} message(s)", d["messages"].as_array().map_or(0, Vec::len)),
            "turn_failed" => format!(
                "TURN FAILED ({} in a row, next try in {}s): {}",
                d["failures"],
                d["retry_in_s"],
                clip(&text("error"), 200)
            ),
            "turn_interrupted" => "turn interrupted; its mail waits".to_string(),
            "spawn" => format!("created by {}", label(conn, &text("parent"))),
            "stop" => format!("stopped by {}", label(conn, &text("by"))),
            "tag" => {
                let set = d["set"].as_object().map(|set| {
                    set.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join(", ")
                });
                format!("tagged by {}: {}", label(conn, &text("by")), set.unwrap_or_default())
            }
            "remove" => {
                let cost = d["cost_usd"].as_f64().map(|c| format!(", had cost ${c:.3}")).unwrap_or_default();
                format!("removed {} (was at {}{cost})", text("name"), text("commit"))
            }
            "start" => format!("started by {}", label(conn, &text("by"))),
            "commit" => format!("commit {} {}", text("commit").chars().take(8).collect::<String>(), text("subject")),
            "commit_skipped" => format!("nothing committed: {}", text("why")),
            "handled_by_code" => format!("its handler dealt with {} message(s)", d["messages"].as_array().map_or(0, Vec::len)),
            "run_start" => "service started (.jarvis/run)".to_string(),
            "run_exit" => format!("service exited (.jarvis/run): {}", text("status")),
            "kernel_error" => format!("KERNEL PROBLEM: {}", clip(&text("error"), 200)),
            "step" => format!("  · {:<8} {}", text("step"), clip(&text("text"), 160)),
            other if d == json!({}) => other.to_string(),
            other => format!("{other} {}", clip(&data, 160)),
        };
        lines.push(format!("{time} {:<16} {what}", label(conn, &proc)));
    }
    Ok(lines)
}

fn log(args: cli::Log) -> Res<()> {
    let conn = Home::find()?.open()?;
    let who = match &args.who {
        Some(who) => Some(db::resolve(&conn, who)?),
        None => None,
    };
    let who = who.as_deref();
    let count = args.count.unwrap_or(if args.json { 500 } else { 30 });
    // Start just before the oldest of the newest `count` facts.
    let window: i64 = conn.query_row(
        &format!("SELECT COALESCE(MIN(id), 0) - 1 FROM (SELECT id FROM events WHERE id > ?1 AND {ABOUT_PROC} AND {STEPS} AND (?5 IS NULL OR id < ?5) ORDER BY id DESC LIMIT ?4)"),
        params![args.after.unwrap_or(0), who, args.steps, count, args.before],
        |r| r.get(0),
    )?;
    let mut last = window.max(args.after.unwrap_or(0));
    if args.json {
        let mut stmt = conn.prepare(&format!("SELECT id, at, proc, kind, data FROM events WHERE id > ?1 AND {ABOUT_PROC} AND {STEPS} AND (?4 IS NULL OR id < ?4) ORDER BY id"))?;
        let rows: Vec<Value> = stmt
            .query_map(params![last, who, args.steps, args.before], |r| {
                let data: String = r.get(4)?;
                Ok(json!({
                    "id": r.get::<_, i64>(0)?, "at": r.get::<_, i64>(1)?, "proc": r.get::<_, String>(2)?,
                    "kind": r.get::<_, String>(3)?, "data": serde_json::from_str::<Value>(&data).unwrap_or(Value::Null),
                }))
            })?
            .collect::<Result<_, _>>()?;
        println!("{}", Value::Array(rows));
        return Ok(());
    }
    let mut printed = false;
    loop {
        let after = last;
        for line in event_lines_before(&conn, after, args.before, who, args.steps, &mut last)? {
            println!("{line}");
            printed = true;
        }
        if !args.follow {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    if !printed {
        println!("nothing has happened yet");
    }
    Ok(())
}
