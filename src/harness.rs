//! Harness adapters: how a kernel runs a process's agent.
//!
//! An adapter runs one turn of one kind of agent. The kernel starts it as a
//! program of its own, `jarvis harness NAME`, so a fault in an adapter is a
//! failed turn and nothing more. The contract is the same for every adapter,
//! so the kernel knows nothing about any of them:
//!
//! ```text
//!   in   standard input        the prompt (the process's new messages)
//!        working directory     the process's own directory
//!        $JARVIS_ID            the process's address
//!        $JARVIS_SESSION       the session its last turn ended with, or empty
//!        $JARVIS_AGENT_DIR     a directory of this process's own, in the store,
//!                              for what its agent must have again next turn. It
//!                              is the same on every machine that uses the store,
//!                              and it is deleted when the process is removed
//!        $JARVIS_GUIDE         path to the guide every agent is given
//!        $JARVIS_TAGS          the process's tags, as a JSON object. One is read
//!                              by every adapter: `model`, the model this turn is
//!                              to run on, in whatever names its agent takes
//!   out  standard output       JSON objects, one per line.
//!                              While the agent works, one line per step, which
//!                              the kernel records at once (`jarvis log --steps`):
//!                                {"step": what kind of step,
//!                                 "text": the step, for a person to read,
//!                                 ...anything else worth recording}
//!                              Then, as the last line, how the turn ended:
//!                                {"session": id to continue next time,
//!                                 "reply":   what the agent said last,
//!                                 "model":   the model its tag asked for, or null,
//!                                 "cost_usd": what this turn cost, if known; or
//!                                 "cost_usd_session": what the whole session has
//!                                            cost so far, from which the kernel
//!                                            works out this turn's share,
//!                                 ...anything else worth recording (time)}
//!        exit code             non-zero if the agent could not run
//! ```
//!
//! To support another kind of agent, add a function here and its name to
//! `run` and `known`.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The agents a process can be given with `--harness`, as shown to people.
pub const AGENTS: &str = "claude";

/// An adapter for tests: no model, it answers every message.
const ECHO: &str = "echo";

pub fn known(name: &str) -> bool {
    name == "claude" || name == ECHO
}

/// Run one turn. What is returned is the exit code.
pub fn run(name: &str) -> i32 {
    let done = match name {
        "claude" => claude(),
        ECHO => echo(),
        _ => Err(format!("there is no harness `{name}`. There is: {AGENTS}")),
    };
    match done {
        Ok(()) => 0,
        Err(problem) => {
            eprintln!("{}", problem.trim_end());
            1
        }
    }
}

fn given(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("${name} is not set. `jarvis harness` is run by a kernel, for a turn of one of its processes"))
}

/// The model the process's tag `model` asks for, if it has one.
fn model_asked() -> Option<String> {
    let tags: Value =
        serde_json::from_str(&std::env::var("JARVIS_TAGS").unwrap_or_default()).unwrap_or_default();
    tags["model"]
        .as_str()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string)
}

fn emit(value: Value) {
    println!("{value}");
    let _ = std::io::stdout().flush();
}

// ---------------------------------------------------------------- Claude Code
//
// What the agent may do is decided in two places.
//
//   Every agent     the flags below: edit files in its own directory, use the
//                   `jarvis` command, use all of git, search the web. Set
//                   $JARVIS_CLAUDE_FLAGS to replace them (for example
//                   `--dangerously-skip-permissions` to allow any command).
//   Each process    `.jarvis/claude.json` in the process's directory, if it
//                   exists: a Claude Code settings file. It is how a process is
//                   given the tools its work needs, for example
//                     {"permissions": {"allow": ["Bash(npm:*)", "Bash(cargo:*)"]}}
//                   It is part of the process's code, so forks start with it and
//                   the agent can change it. It is read at the start of each turn.
//
// The settings are handed to Claude Code as text, not as a file. Claude Code
// refuses to let an agent edit a file it was given as settings, and ignores a
// `.claude/settings.json` in a directory nobody has approved by hand.
//
// What is kept. Claude Code is given $JARVIS_AGENT_DIR as its whole
// configuration directory, so the agent's session is a file there and nowhere
// else. The session's id is worked out from the process's address, so the
// address alone finds it: each turn continues that session, and makes it if
// it is not there. Nothing of the machine's own `~/.claude` is used, the
// login included: the kernel must have been started with
// $CLAUDE_CODE_OAUTH_TOKEN set, to a token made by `claude setup-token`.

/// The most of a tool's input or output that is recorded for one step.
const MOST: usize = 4000;

const DEFAULT_FLAGS: &[&str] = &[
    "--permission-mode",
    "acceptEdits",
    "--allowedTools",
    "Bash(jarvis:*)",
    "Bash(git:*)",
    "Bash(ls:*)",
    "Bash(cat:*)",
    "Read",
    "Edit",
    "Write",
    "Glob",
    "Grep",
    "WebSearch",
    "WebFetch",
];

const NOT_LOGGED_IN: &str = "Claude is not logged in. An agent's sessions are kept in the store, and \
    the login of this machine is not there, so the kernel must be started \
    with CLAUDE_CODE_OAUTH_TOKEN set to a token made by `claude setup-token`. \
    Stop the kernel and start it again from a shell that has it.";

/// Claude Code deletes sessions that were not used for this many days. It has
/// no way to say never, so: a hundred years.
const KEEP_DAYS: u32 = 36500;

const LOST: &str = "[jarvis] Your earlier session could not be continued, so you remember \
    nothing from it. Read what is in your .jarvis directory before acting.\n\n";

const OWN_SETTINGS: &str = ".jarvis/claude.json";

/// The session of an address: the same address always gives the same one.
fn session_of(address: &str) -> String {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_URL,
        format!("jarvis:{address}").as_bytes(),
    )
    .to_string()
}

/// The process's own settings as text, and a notice if they are unusable.
fn own_settings() -> (Option<String>, String) {
    let text = match std::fs::read_to_string(OWN_SETTINGS) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (None, String::new()),
        Err(e) => {
            return (
                None,
                format!(
                    "[jarvis] `{OWN_SETTINGS}` could not be read ({e}), so it was not used.\n\n"
                ),
            );
        }
    };
    let problem = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(_)) => return (Some(text), String::new()),
        Ok(_) => "it is not a JSON object".to_string(),
        Err(e) => e.to_string(),
    };
    (
        None,
        format!(
            "[jarvis] `{OWN_SETTINGS}` is not valid ({problem}), so it was not used this \
             turn: you have only the tools every agent has. Fix the file.\n\n"
        ),
    )
}

fn clip(text: &str) -> String {
    let length = text.chars().count();
    if length <= MOST {
        return text.to_string();
    }
    format!(
        "{}\n… ({} more characters)",
        text.chars().take(MOST).collect::<String>(),
        length - MOST
    )
}

/// One line saying what a tool was used for.
fn tool_line(name: &str, input: &Value) -> String {
    let here = std::env::current_dir()
        .map(|d| format!("{}/", d.display()))
        .unwrap_or_default();
    for key in [
        "command",
        "file_path",
        "pattern",
        "query",
        "url",
        "path",
        "prompt",
        "description",
    ] {
        if let Some(value) = input
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            let value = if here.is_empty() {
                value
            } else {
                value.strip_prefix(&here).unwrap_or(value)
            };
            return format!("{name}: {value}");
        }
    }
    match input.as_object() {
        Some(fields) if !fields.is_empty() => format!("{name}: {input}"),
        _ => name.to_string(),
    }
}

fn result_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let parts: Vec<String> = content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part.is_object())
                .map(|part| match part["type"].as_str() {
                    Some("text") => part["text"].as_str().unwrap_or("").to_string(),
                    kind => format!("[{}]", kind.unwrap_or("something")),
                })
                .collect()
        })
        .unwrap_or_default();
    parts.join("\n")
}

/// The steps in one line of Claude Code's output.
fn steps(event: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    if !matches!(event["type"].as_str(), Some("assistant" | "user")) {
        return found;
    }
    let Some(content) = event["message"]["content"].as_array() else {
        return found;
    };
    // Set when the step was taken by a helper the agent started with a tool.
    let inside = event["parent_tool_use_id"]
        .as_str()
        .filter(|id| !id.is_empty());
    for part in content {
        let mut step = match part["type"].as_str() {
            Some("text") if !part["text"].as_str().unwrap_or("").trim().is_empty() => {
                json!({"step": "text", "text": part["text"]})
            }
            Some("thinking") if !part["thinking"].as_str().unwrap_or("").trim().is_empty() => {
                json!({"step": "thinking", "text": part["thinking"]})
            }
            Some("tool_use") => {
                let input = if part["input"].is_object() {
                    part["input"].clone()
                } else {
                    json!({})
                };
                let name = part["name"].as_str().unwrap_or("?");
                json!({
                    "step": "tool",
                    "text": clip(&tool_line(name, &input)),
                    "tool": part["name"],
                    "input": if input.to_string().chars().count() <= MOST { input } else { Value::Null },
                    "id": part["id"],
                })
            }
            Some("tool_result") => json!({
                "step": "result",
                "text": clip(&result_text(&part["content"])),
                "error": part["is_error"].as_bool().unwrap_or(false),
                "id": part["tool_use_id"],
            }),
            _ => continue,
        };
        if let Some(inside) = inside {
            step["inside"] = json!(inside);
        }
        found.push(step);
    }
    found
}

/// Words as a shell would split them: spaces part them, quotes and `\` keep
/// them together.
fn words(text: &str) -> Vec<String> {
    let (mut all, mut word, mut open, mut quote) = (Vec::new(), String::new(), false, None::<char>);
    let mut letters = text.chars();
    while let Some(c) = letters.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => word.extend(letters.next()),
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                open = true;
            }
            (None, c) if c.is_whitespace() => {
                if open || !word.is_empty() {
                    all.push(std::mem::take(&mut word));
                }
                open = false;
            }
            (None, c) => word.push(c),
        }
    }
    if open || !word.is_empty() {
        all.push(word);
    }
    all
}

/// How one run of Claude Code ended: its exit code, the final result it
/// reported if it did, and everything else it printed, for error messages.
struct Ran {
    failed: bool,
    result: Option<Value>,
    said: String,
    /// The models that spoke as the agent itself, in the order they first did.
    models: Vec<String>,
}

impl Ran {
    fn has(&self, text: &str) -> bool {
        self.said.contains(text)
            || self
                .result
                .as_ref()
                .is_some_and(|r| r.to_string().contains(text))
    }
}

/// Run Claude Code for one turn, printing its steps as they happen. `which`
/// are the flags that say which session: continue one, or make one.
fn claude_once(
    prompt: &str,
    which: &[&str],
    guide: &str,
    settings: Option<&str>,
    model: Option<&str>,
    keep_in: &Path,
) -> Result<Ran, String> {
    let mut command = Command::new("claude");
    command
        .args([
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--append-system-prompt",
            guide,
        ])
        .args(which);
    if let Some(settings) = settings {
        command.args(["--settings", settings]);
    }
    match std::env::var("JARVIS_CLAUDE_FLAGS") {
        Ok(flags) => command.args(words(&flags)),
        Err(_) => command.args(DEFAULT_FLAGS),
    };
    // Last, so that it is this that counts.
    if let Some(model) = model {
        command.args(["--model", model]);
    }
    let mut child = command
        .env("CLAUDE_CONFIG_DIR", keep_in)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start `claude` ({e}). Claude Code must be installed, and on the PATH the kernel was started with"))?;
    // Its input and its errors go on the side, so that it can never be stuck
    // on one of them while its output is being read here.
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let prompt = prompt.to_string();
    let writing = std::thread::spawn(move || {
        let _ = stdin.write_all(prompt.as_bytes());
    });
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let errors = std::thread::spawn(move || {
        let mut all = String::new();
        let _ = stderr.read_to_string(&mut all);
        all
    });
    let (mut result, mut other, mut models) = (None, String::new(), Vec::new());
    for line in BufReader::new(child.stdout.take().expect("stdout is piped")).split(b'\n') {
        let Ok(line) = line else { break };
        let line = String::from_utf8_lossy(&line);
        match serde_json::from_str::<Value>(&line) {
            Ok(event) if event["type"] == "result" => result = Some(event),
            Ok(event) if event.is_object() => {
                // Which model this was, unless it was a helper the agent started.
                if let Some(model) = event["message"]["model"].as_str()
                    && event["type"] == "assistant"
                    && event["parent_tool_use_id"]
                        .as_str()
                        .is_none_or(str::is_empty)
                    && !models.iter().any(|m| m == model)
                {
                    models.push(model.to_string());
                }
                steps(&event).into_iter().for_each(emit)
            }
            _ => {
                other.push_str(&line);
                other.push('\n');
            }
        }
    }
    let status = child
        .wait()
        .map_err(|e| format!("could not wait for `claude`: {e}"))?;
    let _ = writing.join();
    let said = format!("{}{other}", errors.join().unwrap_or_default());
    Ok(Ran {
        failed: !status.success(),
        result,
        said,
        models,
    })
}

fn claude() -> Result<(), String> {
    let mut prompt = String::new();
    std::io::stdin()
        .read_to_string(&mut prompt)
        .map_err(|e| format!("could not read the prompt: {e}"))?;
    let address = given("JARVIS_ID")?;
    // Claude Code keeps everything in the directory of this agent, for good.
    let keep_in = PathBuf::from(given("JARVIS_AGENT_DIR")?);
    std::fs::create_dir_all(&keep_in)
        .map_err(|e| format!("could not create {}: {e}", keep_in.display()))?;
    let kept_for = keep_in.join("settings.json");
    if !kept_for.exists() {
        std::fs::write(
            &kept_for,
            json!({"cleanupPeriodDays": KEEP_DAYS}).to_string(),
        )
        .map_err(|e| format!("could not write {}: {e}", kept_for.display()))?;
    }
    let session = session_of(&address);

    let guide_file = given("JARVIS_GUIDE")?;
    let mut guide = std::fs::read_to_string(&guide_file)
        .map_err(|e| format!("could not read the guide at {guide_file}: {e}"))?;
    // The agent is also shown what `jarvis help` prints, so that the command
    // is described in one place only: its own help.
    guide.push_str("\n## The `jarvis` command\n\nThis is what `jarvis help` prints:\n\n");
    guide.push_str(&crate::cli::overview());

    let (settings, notice) = own_settings();
    let model = model_asked();
    let prompt = format!("{notice}{prompt}");
    let mut ran = claude_once(
        &prompt,
        &["--resume", &session],
        &guide,
        settings.as_deref(),
        model.as_deref(),
        &keep_in,
    )?;
    let mut session_lost = false;
    if ran.failed && ran.has("No conversation found") {
        // There is none yet. If the kernel knows of an earlier turn, there
        // was one, and it is gone.
        session_lost = !std::env::var("JARVIS_SESSION")
            .unwrap_or_default()
            .is_empty();
        let prompt = if session_lost {
            format!("{LOST}{prompt}")
        } else {
            prompt
        };
        ran = claude_once(
            &prompt,
            &["--session-id", &session, "--name", &address],
            &guide,
            settings.as_deref(),
            model.as_deref(),
            &keep_in,
        )?;
    }

    let error = ran
        .result
        .as_ref()
        .is_some_and(|r| r["is_error"].as_bool().unwrap_or(false));
    let out = match ran.result {
        Some(out) if !ran.failed && !error => out,
        result => {
            let mut problem = ran.said.trim().to_string();
            if let Some(out) = result.filter(|_| error) {
                problem = format!(
                    "{}\n{problem}",
                    out["result"]
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| out.to_string())
                );
            }
            if problem.contains("Not logged in") {
                problem = format!("{NOT_LOGGED_IN}\n{problem}");
            }
            if let Some(model) = model.filter(|_| problem.to_lowercase().contains("model")) {
                problem = format!(
                    "Its tag `model` asks for `{model}`. If that is not a model Claude Code knows, \
                     set another with `jarvis tag {address} model=NAME`, or remove the tag with \
                     `jarvis tag {address} model=`.\n{problem}"
                );
            }
            return Err(if problem.trim().is_empty() {
                "claude produced no output".into()
            } else {
                problem
            });
        }
    };

    emit(json!({
        "session": out["session_id"],
        "reply": out["result"].as_str().unwrap_or(""),
        "harness": "claude",
        "model": model,
        // The models that did answer, as Claude Code named them.
        "models": ran.models,
        // Claude Code reports what the session has cost in all, not this turn.
        "cost_usd_session": out["total_cost_usd"],
        "duration_ms": out["duration_ms"],
        "turns": out["num_turns"],
        "session_lost": session_lost,
    }));
    Ok(())
}

// ------------------------------------------------------------------ for tests

/// No model. It answers each message it is given with `ack <message>`, sent
/// to the address the message came from, and keeps one thing between turns:
/// how many turns it has had.
fn echo() -> Result<(), String> {
    let mut prompt = String::new();
    std::io::stdin()
        .read_to_string(&mut prompt)
        .map_err(|e| e.to_string())?;
    let kept = PathBuf::from(given("JARVIS_AGENT_DIR")?);
    std::fs::create_dir_all(&kept).map_err(|e| e.to_string())?;
    let turns: u64 = std::fs::read_to_string(kept.join("turns"))
        .ok()
        .and_then(|t| t.trim().parse().ok())
        .unwrap_or(0);
    std::fs::write(kept.join("turns"), (turns + 1).to_string()).map_err(|e| e.to_string())?;

    emit(json!({"step": "text", "text": "Reading my messages."}));
    let jarvis = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut lines = prompt.lines();
    while let Some(line) = lines.next() {
        let Some((message, sender)) = line
            .strip_prefix("--- message ")
            .and_then(|l| l.strip_suffix(" ---"))
            .and_then(|l| l.split_once(" from "))
        else {
            continue;
        };
        let first = lines.next().unwrap_or("");
        // Not what jarvis itself says, and not answers: or two of these would never stop.
        if first.starts_with("[jarvis]")
            || first.starts_with("ack ")
            || sender.starts_with("kernel@")
        {
            continue;
        }
        let _ = Command::new(&jarvis)
            .args(["send", sender, &format!("ack {message}")])
            .output();
    }
    emit(
        json!({"session": "s", "reply": "Answered what was due.", "cost_usd": 0.0, "model": model_asked()}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_split_as_a_shell_would() {
        assert_eq!(
            words("--model haiku  --allowedTools 'Bash(jarvis:*)' \"a b\" c\\ d ''"),
            [
                "--model",
                "haiku",
                "--allowedTools",
                "Bash(jarvis:*)",
                "a b",
                "c d",
                ""
            ]
        );
    }

    #[test]
    fn an_address_always_has_the_same_session() {
        // The id the adapter gave before it was part of this program.
        assert_eq!(session_of("a@t"), "cf412b3b-07e8-55f4-9606-4faa99ae1b95");
    }
}
