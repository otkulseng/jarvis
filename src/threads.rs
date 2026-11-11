//! The history of a topic, fetched from jarvis when a browser opens it: the
//! newest messages, and for a process the facts about it. Older messages and
//! the steps of earlier turns are fetched only when the reader asks.
//!
//! A browser is given this *after* it has started receiving what is new, so
//! the same message or fact can reach it twice. Browsers keep each by its
//! number, so twice is harmless, and nothing can fall between the two.

use crate::jarvis;
use crate::model::{HUMAN, shown};
use serde_json::{Value, json};

/// How many messages are fetched at a time.
pub const PAGE: usize = 100;

fn is(value: &Value, key: &str, wanted: &str) -> bool {
    value[key].as_str() == Some(wanted)
}

/// The messages of a topic, newest last; only those before `before` if given.
pub async fn messages(topic: &str, before: Option<&str>) -> Result<Vec<Value>, String> {
    Ok(fetch(topic, before).await?.iter().map(shown).collect())
}

/// The human's mail: `human@K` on every kernel that can be reached.
async fn inbox(page: &str, before: Option<&str>) -> Result<Vec<Value>, String> {
    let kernels = jarvis::list(&["kernels", "--json"]).await?;
    let mut all = Vec::new();
    for kernel in kernels.iter().filter(|k| k["reachable"] == true) {
        let human = format!("{HUMAN}@{}", kernel["name"].as_str().unwrap_or(""));
        let mut args: Vec<&str> = vec!["messages", &human, "--json", "-n", page];
        if let Some(before) = before {
            args.extend(["--before", before]);
        }
        // A kernel that does not answer now is left out: its mail shows when it does.
        if let Ok(messages) = jarvis::list(&args).await {
            all.extend(messages.into_iter().filter(|m| is(&shown(m), "to", HUMAN)));
        }
    }
    all.sort_by_key(|m| m["at"].as_i64());
    all.dedup_by(|a, b| a["id"] == b["id"]);
    let keep = all.len().saturating_sub(PAGE);
    Ok(all.split_off(keep))
}

/// `before` is a time, in milliseconds since 1970: the `at` of the oldest
/// message already shown.
async fn fetch(topic: &str, before: Option<&str>) -> Result<Vec<Value>, String> {
    let page = PAGE.to_string();
    if topic == "inbox" {
        return inbox(&page, before).await;
    }
    let mut args: Vec<&str> = vec!["messages", "--json", "-n", &page];
    if let Some(before) = before {
        args.extend(["--before", before]);
    }
    if let Some(name) = topic.strip_prefix("process:") {
        args.push(name);
        jarvis::list(&args).await
    } else if let Some((a, b)) = topic.strip_prefix("edge:").and_then(|pair| pair.split_once('|')) {
        // The human is `human@K` on any kernel: ask the process's kernel for
        // its messages, and keep those whose other end is the human.
        let (process, other) = if a == HUMAN { (b, a) } else { (a, b) };
        if other == HUMAN {
            args.push(process);
            let all = jarvis::list(&args).await?;
            Ok(all.into_iter().filter(|m| { let m = shown(m); is(&m, "from", HUMAN) || is(&m, "to", HUMAN) }).collect())
        } else {
            args.extend([process, "--with", other]);
            jarvis::list(&args).await
        }
    } else {
        Err(format!("`{topic}` is not something that has messages"))
    }
}

/// The steps an agent took between two facts (a turn's start and its end).
pub async fn steps(who: &str, after: i64, before: Option<i64>) -> Result<Vec<Value>, String> {
    let after = after.to_string();
    let before = before.map(|b| b.to_string());
    let mut args = vec!["log", "--json", "--steps", "-n", "20000", "--after", &after, who];
    if let Some(before) = &before {
        args.extend(["--before", before]);
    }
    let all = jarvis::list(&args).await?;
    Ok(all.into_iter().filter(|f| is(f, "kind", "step") && is(f, "proc", who)).collect())
}

/// What a browser is given when it opens a topic.
pub async fn open(topic: &str) -> Value {
    let fetched = async {
        let messages = messages(topic, None).await?;
        let mut facts = Vec::new();
        if let Some(name) = topic.strip_prefix("process:") {
            let recent = jarvis::list(&["log", "--json", "-n", "400", name]).await?;
            facts.extend(recent.into_iter().filter(|f| is(f, "proc", name)));
            // If it is in the middle of a turn, what it has done in it so far.
            let ends = ["turn", "turn_failed", "turn_interrupted"];
            let last = facts.iter().rev().find(|f| is(f, "kind", "turn_start") || ends.iter().any(|end| is(f, "kind", end)));
            if let Some(start) = last.filter(|f| is(f, "kind", "turn_start")).and_then(|f| f["id"].as_i64()) {
                facts.extend(steps(name, start, None).await?);
            }
        }
        Ok::<_, String>((messages, facts))
    };
    match fetched.await {
        Ok((messages, facts)) => {
            json!({"type": "thread", "topic": topic, "more": messages.len() >= PAGE, "messages": messages, "facts": facts})
        }
        Err(problem) => json!({"type": "thread", "topic": topic, "problem": problem, "messages": [], "facts": []}),
    }
}
