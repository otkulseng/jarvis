//! One turn of a process: its waiting mail goes to its code, then to its agent.

use super::*;

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
    let _ = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes());
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
        .map(|status| Output {
            status,
            stdout,
            stderr: errors.join().unwrap_or_default(),
        })
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
    let input =
        json!({"id": mail.id, "from": mail.from, "body": mail.body, "attempt": mail.attempt})
            .to_string();
    match run_tracked(
        job,
        conn,
        "handle",
        Command::new(job.dir.join(".jarvis/handle")),
        &input,
        &mut |_| false,
    ) {
        Ok(out) if out.status.success() => Handled::Yes,
        Ok(out) if out.status.code() == Some(3) => Handled::NotMine,
        Ok(out) => Handled::Failed(format!(
            "{}\n{}",
            out.status,
            tail(&String::from_utf8_lossy(&out.stderr), 30)
        )),
        Err(e) => Handled::Failed(e),
    }
}

/// What is said when a process is to have a kind of agent there is none of.
pub(super) fn no_such_harness(name: &str) -> String {
    format!(
        "this jarvis cannot run an agent of the kind `{name}`. It can run: {}. Or give `--harness {NO_AGENT}`, for a process whose code is all there is",
        crate::harness::AGENTS
    )
}

fn run_agent(job: &Job, conn: Option<&Connection>, prompt: &str) -> Result<Value, String> {
    let Some(harness) = &job.harness else {
        return Err("it has no agent".into());
    };
    if !crate::harness::known(harness) {
        return Err(no_such_harness(harness));
    }
    let mut command = Command::new(&job.exe);
    command.args(["harness", harness]);
    command.env("JARVIS_SESSION", job.session.as_deref().unwrap_or(""));
    if let Some(dir) = &job.agent_dir {
        command.env("JARVIS_AGENT_DIR", dir);
    }
    // What the agent does is noted while it does it.
    let mut step = |line: &str| match serde_json::from_str::<Value>(line) {
        Ok(value) if value.get("step").is_some() => {
            let _ = job.tx.send(Event::Step {
                name: job.name.clone(),
                value,
            });
            true
        }
        _ => false,
    };
    let out = run_tracked(job, conn, "agent", command, prompt, &mut step)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(format!(
            "harness exited with {}:\n{}\n{}",
            out.status,
            tail(&String::from_utf8_lossy(&out.stderr), 30),
            tail(&stdout, 10)
        ));
    }
    let value: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        format!(
            "harness did not print a JSON object:\n{}",
            tail(&stdout, 10)
        )
    })?;
    if !value["reply"].is_string() {
        return Err(format!(
            "harness output has no `reply`:\n{}",
            tail(&stdout, 10)
        ));
    }
    Ok(value)
}

/// One turn of a process: its waiting mail goes to its code, then its agent.
pub(super) fn turn(job: Job) -> Done {
    let conn = index::open(&job.db_path).ok();
    let conn = conn.as_ref();
    let cancelled = || job.cancel.load(Ordering::SeqCst);
    let has_handle = is_executable(&job.dir.join(".jarvis/handle"));
    let (mut by_code, mut for_agent, mut code_failed, mut prompt) =
        (Vec::new(), Vec::new(), Vec::new(), String::new());
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
        prompt.push_str(&format!(
            "--- message {} from {} ---\n{}\n",
            mail.id, mail.from, mail.body
        ));
        if mail.attempt > 1 {
            prompt.push_str(&format!(
                "[jarvis] This message was given to you before (this is attempt {}), and that turn did not finish. \
                 You may already have acted on it: check what you did before repeating anything.\n",
                mail.attempt
            ));
        }
        if let Some(error) = failure {
            prompt.push_str(&format!(
                "[jarvis] Your .jarvis/handle failed on this message:\n{error}\n"
            ));
            code_failed.push((mail.id.clone(), error));
        }
        prompt.push('\n');
    }
    let agent = if cancelled() {
        Agent::Interrupted
    } else if for_agent.is_empty() {
        Agent::NotNeeded
    } else if job.harness.is_none() {
        Agent::Absent
    } else {
        let mut header = format!(
            "[jarvis] You are `{}`: the process `{}` on the kernel `{}`. {} new message(s). \
             To answer one, send to the address it is from.\n",
            job.addr,
            job.name,
            job.kernel,
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
    Done {
        name: job.name,
        by_code,
        for_agent,
        code_failed,
        agent,
    }
}
