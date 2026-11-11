//! Talking to jarvis. The page knows jarvis only through the `jarvis`
//! command: one-off questions here, and the change feed (`jarvis watch`).

use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

/// Run `jarvis` with the given arguments (and optional standard input) and
/// return what it printed. `as_human`: the command is the human's doing, not
/// this process's.
pub async fn run(args: &[&str], input: Option<&str>, as_human: bool) -> Result<String, String> {
    let mut command = Command::new("jarvis");
    command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if as_human {
        command.env_remove("JARVIS_ID");
    }
    let mut child = command.spawn().map_err(|e| format!("could not run jarvis: {e}"))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    if let Some(input) = input {
        stdin.write_all(input.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    drop(stdin);
    let out = child.wait_with_output().await.map_err(|e| e.to_string())?;
    // 2: jarvis did part of it (a kernel did not answer, or a request to
    // another kernel is not carried out yet). What it printed still counts.
    if !out.status.success() && out.status.code() != Some(2) {
        return Err(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("jarvis: ").to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a `jarvis ... --json` command and return the list it printed.
pub async fn list(args: &[&str]) -> Result<Vec<Value>, String> {
    let text = run(args, None, false).await?;
    match serde_json::from_str(&text) {
        Ok(Value::Array(rows)) => Ok(rows),
        _ => Err(format!("`jarvis {}` did not print a JSON list", args.join(" "))),
    }
}

/// Follow every kernel that can be reached (`jarvis watch --all`) for as
/// long as this program runs, handing each line to `lines`. If the command
/// ends (jarvis was reinstalled, say) it is started again from the last fact
/// seen of each kernel, so nothing is missed.
pub async fn watch(lines: mpsc::Sender<Value>) {
    let mut cursors: std::collections::BTreeMap<String, i64> = Default::default();
    loop {
        let after: Vec<String> = cursors.iter().map(|(kernel, n)| format!("{kernel}:{n}")).collect();
        let mut args = vec!["watch", "--all", "--steps"];
        for cursor in &after {
            args.extend(["--after", cursor]);
        }
        let child = Command::new("jarvis").args(&args).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(true).spawn();
        match child {
            Ok(mut child) => {
                let mut reader = BufReader::new(child.stdout.take().expect("stdout is piped")).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                    if let (Some(kernel), Some(id)) = (value["kernel"].as_str(), value["fact"]["id"].as_i64().or_else(|| value["cursor"].as_i64())) {
                        cursors.insert(kernel.to_string(), id);
                    }
                    if lines.send(value).await.is_err() {
                        return;
                    }
                }
                let _ = child.wait().await;
                eprintln!("`jarvis watch` ended; starting it again");
            }
            Err(e) => eprintln!("could not run `jarvis watch`: {e}"),
        }
        // Tell the model that what it knows may be stale.
        if lines.send(serde_json::json!({"kind": "restart"})).await.is_err() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
