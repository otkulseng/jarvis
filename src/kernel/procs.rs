//! Looking after the processes a kernel hosts: their services, their turns,
//! and creating, stopping and removing them.

use super::*;

impl Kernel {
    /// Environment every program of a process gets: who it is, which kernel
    /// carries out its `jarvis` commands, and its name on the commits it makes.
    pub(super) fn proc_env(&self, name: &str) -> Vec<(String, String)> {
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
            ("GIT_AUTHOR_NAME".into(), name.to_string()),
            ("GIT_COMMITTER_NAME".into(), name.to_string()),
            ("PATH".into(), path),
        ]
    }

    /// Look after one process: its service, and its mail.
    pub(super) fn tend(&mut self, name: &str) {
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
    pub(super) fn put_to_rest(&mut self, name: &str) {
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
    pub(super) fn end_run(&mut self, name: &str) {
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
    pub(super) fn tend_run(&mut self, proc: &Proc, dir: &Path) -> Res<()> {
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
                self.tell(&proc.addr, &format!("[jarvis] Your .jarvis/run could not be started: {e}"))?;
            }
        }
        Ok(())
    }

    /// A process's service has ended.
    pub(super) fn run_exit(&mut self, name: &str, pid: u32, status: &str, ok: bool) {
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
                if let Err(e) = self.tell(&addr, &notice) {
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
    pub(super) fn dispatch(&mut self, proc: &Proc, dir: &Path) -> Res<()> {
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

    pub(super) fn release(&mut self, name: &str) {
        if let Some(live) = self.live.get_mut(name) {
            live.busy = false;
            live.cancel = None;
            live.turn = None;
        }
    }

    /// A turn is over.
    pub(super) fn finish(&mut self, done: Done) {
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
    pub(super) fn record(&mut self, done: Done) -> Res<()> {
        let name = done.name.as_str();
        let proc = self.find(name)?.ok_or("the process is no longer there")?;
        let addr = proc.addr.as_str();
        if !done.by_code.is_empty() {
            self.rec(addr, "handled_by_code", json!({"messages": done.by_code}))?;
            for id in &done.by_code {
                self.receipt(id, "handled", json!({}));
            }
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
                // A harness that can only say what its session has cost in
                // all says that, and this turn's share is worked out here.
                if let (Some(total), Some(session)) = (out["cost_usd_session"].as_f64(), out["session"].as_str()) {
                    let before: f64 = self
                        .db
                        .query_row(
                            "SELECT COALESCE(MAX(json_extract(data, '$.cost_usd_session')), 0) FROM records
                             WHERE proc = ?1 AND kind = 'turn' AND json_extract(data, '$.session') = ?2",
                            params![addr, session],
                            |r| r.get(0),
                        )
                        .unwrap_or(0.0);
                    out["cost_usd"] = json!(if total >= before { total - before } else { total });
                }
                self.rec(addr, "turn", out)?;
                for id in &done.for_agent {
                    self.receipt(id, "handled", json!({}));
                }
                reply.lines().find(|l| !l.trim().is_empty()).unwrap_or("turn").chars().take(72).collect()
            }
            Agent::NoAgent => {
                // Nobody is there to read these: they go back to whoever sent them.
                let why = format!("`{addr}` has no agent, and its code did not take the message");
                for id in &done.for_agent {
                    self.undeliverable(id, &why)?;
                }
                self.rec(addr, "turn_end", json!({}))?;
                "handled by code".into()
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
                // Its work is in its directory but not in the repository: it should know.
                self.tell(addr, &format!("[jarvis] What you changed in your last turn could not be committed, so it is in your directory only and not safe yet. Look at `git status` there and put it right; the next turn tries again.\n\nThe error:\n{e}"))?;
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
    pub(super) fn tell_senders(&mut self, proc: &Proc, messages: &[String], error: &str) -> Res<()> {
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
            self.tell(&sender, &notice)?;
        }
        Ok(())
    }

    /// The commit a process is to start from. What the repository does not
    /// have is fetched from the remote, once.
    pub(super) fn commit_for(&self, what: &str) -> Res<String> {
        let repo = &self.cfg.repo;
        let at_remote = format!("origin/{what}");
        if let Some(commit) = repo.commit_of(what).or_else(|| repo.commit_of(&at_remote)) {
            return Ok(commit);
        }
        let fetched = repo.fetch(what);
        if let Some(commit) = repo.commit_of(&at_remote) {
            return Ok(commit);
        }
        Err(format!(
            "`{what}` is not a branch, tag or commit in the repository of the kernel `{}` ({}), and fetching a branch of that name from its remote `origin` did not bring one{}. \
             Push it to that remote from where it is (`git push origin {what}`), then try again",
            self.me,
            repo.root.display(),
            match fetched {
                Ok(()) => String::new(),
                // Git's own words, without the command that was run.
                Err(e) => format!(" ({})", e.to_string().rsplit("failed: ").next().unwrap_or("").trim()),
            }
        )
        .into())
    }

    pub(super) fn spawn(&mut self, args: cli::Spawn, by: &str, prompt: Option<String>) -> Res<Value> {
        let (name, kernel) = index::address(&args.address, "process")?;
        if kernel != self.me {
            return Err(format!("`{}` is to be on the kernel `{kernel}`, and this is `{}`", args.address, self.me).into());
        }
        index::check_name(name, "process")?;
        if name == HUMAN || name == KERNEL {
            return Err(format!("`{name}` is reserved; choose another name").into());
        }
        let addr = args.address.clone();
        let branch = addr.clone();
        let no_agent = args.harness == NO_AGENT;
        if let Some(existing) = self.find(name)? {
            if existing.status == "alive" {
                return Err(format!("`{addr}` is already running; a kernel runs a name once (`jarvis ps {addr}` shows it)").into());
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
        if continued && args.from.is_some() {
            return Err(format!(
                "the branch `{branch}` exists, so `{addr}` carries on from it, and --from cannot be used as well. \
                 To start afresh under this name, delete that branch first (`git branch -D {branch}`)"
            )
            .into());
        }
        if no_agent && prompt.is_some() {
            return Err("a process without an agent has nobody to read a prompt. Leave the prompt out, or choose a harness".into());
        }
        let start = match &args.from {
            None => None,
            Some(what) => Some(self.commit_for(what)?),
        };
        // It starts from another process's branch: a fork of that process.
        let forked: Option<&String> = args.from.as_ref().filter(|what| what.contains('@'));
        let blank = prompt.is_none() && start.is_none() && !continued;
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
            if let Some(prompt) = &prompt {
                fs::create_dir_all(dir.join(".jarvis"))?;
                fs::write(dir.join(".jarvis/PROMPT.md"), format!("{}\n", prompt.trim_end()))?;
            }
            let message = format!("{name}: {} on {} by {by}", if continued { "carried on" } else { "spawned" }, kernel.me);
            if git::commit_all(&dir, name, &message)?.is_none() && !continued {
                git::commit_empty(&dir, name, &message)?;
            }
            kernel.rec(&addr, "spawn", json!({"parent": by, "from": args.from, "harness": args.harness, "branch": branch, "continued": continued, "tags": tags}))?;
            Ok(())
        };
        if let Err(e) = fill(self) {
            let _ = git::worktree_remove(&repo, &dir);
            if !continued {
                let _ = repo.git(&["branch", "-D", &branch]);
            }
            return Err(format!("could not create `{addr}`: {e}").into());
        }
        self.live.remove(name);

        // A notice from its kernel wakes its agent. A blank process gets
        // none: it waits for someone to tell it what to be. A process without
        // an agent gets none either: there is nobody to read it.
        if !blank && !no_agent {
            let has_prompt = dir.join(".jarvis/PROMPT.md").exists();
            let read = if has_prompt { "Your instructions are in .jarvis/PROMPT.md. Read them and begin." } else { "Look at what is in your directory and begin." };
            // Said when it starts from a branch that is not a process: an expert.
            let is_branch = |what: &str| ["refs/heads", "refs/remotes/origin"].iter().any(|place| repo.git(&["rev-parse", "--verify", "--quiet", &format!("{place}/{what}")]).is_ok());
            let hello = if continued {
                format!(
                    "[jarvis] You have been started by {by}, carrying on from your branch `{branch}`. Your directory is as that \
                     branch was. The conversation you had before did not come along: your notes in `.jarvis/` and \
                     `jarvis messages {addr}` hold what was said. Look at both, and carry on."
                )
            } else {
                match (forked, &args.from) {
                    (Some(origin), _) if name_of(origin) == name => format!(
                        "[jarvis] You have been started by {by} as a continuation of `{origin}`: your directory began as a copy of \
                         its branch, notes included. You are `{addr}` now, with a branch of your own. The conversation `{origin}` \
                         had did not come along: your notes in `.jarvis/` and `jarvis messages {origin}` hold what was said. Look \
                         at both, and carry on."
                    ),
                    (Some(origin), _) => format!(
                        "[jarvis] You have just been created by {by} as a fork of `{origin}`: your directory began as a copy of its \
                         branch, notes and prompt included, but you are a new process, `{addr}`, with a memory of your own. {}",
                        if prompt.is_some() { read.to_string() } else { format!("You were given no instructions of your own. Look at what is in your directory, and ask {by} what you are for.") }
                    ),
                    (None, Some(what)) if is_branch(what) => format!(
                        "[jarvis] You have just been created by {by}. Your directory began as a copy of the branch `{what}`: that is \
                         your expert, and where your finished work goes back to (the guide says how). {read}"
                    ),
                    (None, Some(what)) => format!("[jarvis] You have just been created by {by}. Your directory began as a copy of `{what}`. {read}"),
                    (None, None) => format!("[jarvis] You have just been created by {by}. {read}"),
                }
            };
            self.tell(&addr, &hello)?;
        }
        let mut notes = Vec::new();
        match self.cfg.harness(&args.harness) {
            Some(adapter) if !is_executable(&adapter) => notes.push(format!(
                "there is no harness adapter `harness/{}` in {}, so its agent cannot run until there is",
                args.harness,
                self.cfg.repo.root.display()
            )),
            None if !is_executable(&dir.join(".jarvis/handle")) => {
                notes.push(format!("`{addr}` has no agent and no `.jarvis/handle`, so nothing reads its messages: each comes back to its sender as not read"))
            }
            _ => {}
        }
        if blank && !no_agent {
            notes.push(format!("`{addr}` is blank: it does nothing until it gets a message (`jarvis send {addr} ...`)"));
        }
        self.tend(name);
        Ok(said(vec![addr], notes))
    }

    /// A process of this kernel, from its address.
    pub(super) fn local(&self, who: &str) -> Res<Proc> {
        let (name, kernel) = index::address(who, "process")?;
        if kernel != self.me {
            return Err(format!("`{who}` is on the kernel `{kernel}`, and this is `{}`", self.me).into());
        }
        self.find(name)?.ok_or_else(|| format!("there is no process `{who}` (`jarvis ps @{}` lists the processes of this kernel)", self.me).into())
    }

    pub(super) fn set_status(&mut self, who: &[String], alive: bool, by: &str) -> Res<Value> {
        // All of them must exist before any is touched.
        let procs: Vec<Proc> = who.iter().map(|who| self.local(who)).collect::<Res<_>>()?;
        let word = if alive { "alive" } else { "stopped" };
        let mut lines = Vec::new();
        for proc in procs {
            if proc.status == word {
                lines.push(format!("{} was already {word}", proc.addr));
                continue;
            }
            self.rec(&proc.addr, if alive { "start" } else { "stop" }, json!({"by": by}))?;
            lines.push(format!("{} is {word}", proc.addr));
            self.tend(&proc.name);
        }
        Ok(said(lines, vec![]))
    }

    pub(super) fn tag(&mut self, who: &str, tags: &[(String, String)], by: &str) -> Res<Value> {
        let proc = self.local(who)?;
        let set: serde_json::Map<String, Value> = tags.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
        self.rec(&proc.addr, "tag", json!({"by": by, "set": set}))?;
        Ok(said(vec![], vec![]))
    }

    /// Begin removing a process. It is finished, and answered, once nothing
    /// of the process runs any more.
    pub(super) fn remove(&mut self, who: &str, keep_branch: bool, by: &str, reply: Sender<Result<Value, String>>) -> Res<()> {
        let proc = self.local(who)?;
        if proc.addr == by {
            return Err("a process cannot remove itself: this command is one of the programs that would be ended. Ask another process, or the human".into());
        }
        let live = self.live.entry(proc.name.clone()).or_default();
        if live.removing.is_some() {
            return Err(format!("{} is being removed already", proc.addr).into());
        }
        live.removing = Some(Removal { keep_branch, by: by.to_string(), reply });
        self.put_to_rest(&proc.name);
        self.finish_removal(&proc.name);
        Ok(())
    }

    pub(super) fn finish_removal(&mut self, name: &str) {
        let Some(live) = self.live.get(name) else { return };
        if live.busy || live.run.is_some() {
            return;
        }
        let Some(removal) = self.live.remove(name).and_then(|l| l.removing) else { return };
        let done = (|| -> Res<Value> {
            let proc = self.find(name)?.ok_or("it is no longer there")?;
            let addr = proc.addr.clone();
            let repo = self.cfg.repo.clone();
            let dir = self.cfg.proc_dir(name);
            let commit = repo.git(&["rev-parse", "--short", &proc.branch]).ok();
            let (turns, cost): (i64, Option<f64>) = self.db.query_row(
                "SELECT COUNT(*), SUM(json_extract(data, '$.cost_usd')) FROM records WHERE proc = ?1 AND kind = 'turn'",
                [&addr],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            // Mail that nobody will read now.
            let unread: Vec<String> = {
                let mut stmt = self.db.prepare("SELECT id FROM messages WHERE recipient = ?1 AND status IN ('pending', 'taken')")?;
                stmt.query_map([&addr], |r| r.get(0))?.collect::<Result<_, _>>()?
            };
            if dir.exists() {
                git::worktree_remove(&repo, &dir).map_err(|e| format!("could not delete the directory of {addr}, so it was not removed: {e}"))?;
            }
            if !removal.keep_branch && commit.is_some() {
                repo.git(&["branch", "-D", &proc.branch])?;
            }
            let _ = fs::remove_file(self.cfg.run_log(name));
            self.rec(&addr, "remove", json!({"by": removal.by, "commit": commit, "branch_kept": removal.keep_branch, "turns": turns, "cost_usd": cost}))?;
            for id in unread {
                self.receipt(&id, "dropped", json!({}));
            }
            Ok(said(
                vec![match (&commit, removal.keep_branch) {
                    (Some(commit), true) => format!("removed {addr}; its branch stays, at {commit}"),
                    (Some(commit), false) => {
                        format!("removed {addr} (its branch was at {commit}; `git branch {addr} {commit}` brings its files back while git still has them)")
                    }
                    (None, _) => format!("removed {addr}"),
                }],
                vec![],
            ))
        })();
        let _ = removal.reply.send(done.map_err(|e| e.to_string()));
    }
}
