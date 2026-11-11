//! The server behind the web page. It serves the built page and answers the
//! page's questions, all by running the `jarvis` command. It keeps no state.
//!
//!   GET  /api/snapshot?messages_after=N&events_after=M
//!        who exists, and the messages and facts newer than the given ids
//!        (facts include every step agents take inside their turns), and
//!        which build of the page this server is serving
//!   POST /api/send   {"to": ..., "text": ...}       a message from the human
//!   POST /api/act    {"action": "stop" | "start" | "remove", "who": ...}
//!   GET  /api/files?who=NAME                         the files of a process
//!   GET  /api/file?who=NAME&path=PATH                one of them, as text
//!
//! Set by `.jarvis/run`: $PAGE_DIST (the built page, default `web/dist`) and
//! $PAGE_BUILD (the name of this build; the open page reloads itself when it
//! changes).
//!
//! The port: $PORT if set. Otherwise the one this process used last time
//! (`.build/port`), and the first time, the first free one from 7878 up. So
//! the first page process gets 7878 and keeps it, and others get their own.

use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use tower_http::services::ServeDir;

type Failure = (StatusCode, String);
type Params = Query<HashMap<String, String>>;

const FIRST_PORT: u16 = 7878;
const PORT_FILE: &str = ".build/port";
/// The most of one file that is sent to the page.
const MOST: usize = 400_000;

fn fail(text: impl Into<String>) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, text.into())
}

fn bad(text: impl Into<String>) -> Failure {
    (StatusCode::BAD_REQUEST, text.into())
}

/// Run `jarvis` with the given arguments (and optional standard input) and
/// return what it printed. `as_human`: the command is the human's doing, not
/// this process's.
async fn jarvis(args: &[&str], input: Option<&str>, as_human: bool) -> Result<String, Failure> {
    let mut command = Command::new("jarvis");
    command.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if as_human {
        command.env_remove("JARVIS_ID");
    }
    let mut child = command.spawn().map_err(|e| fail(format!("could not run jarvis: {e}")))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    if let Some(input) = input {
        stdin.write_all(input.as_bytes()).await.map_err(|e| fail(e.to_string()))?;
    }
    drop(stdin);
    let out = child.wait_with_output().await.map_err(|e| fail(e.to_string()))?;
    if !out.status.success() {
        return Err(fail(String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("jarvis: ").to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn jarvis_json(args: &[&str]) -> Result<Value, Failure> {
    let text = jarvis(args, None, false).await?;
    serde_json::from_str(&text).map_err(|e| fail(format!("jarvis printed bad JSON: {e}")))
}

fn me() -> String {
    std::env::var("JARVIS_ID").unwrap_or_default()
}

async fn snapshot(Query(q): Params) -> Result<Json<Value>, Failure> {
    let after = |key: &str| q.get(key).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).to_string();
    let (messages_after, events_after) = (after("messages_after"), after("events_after"));
    let messages_args = ["messages", "--json", "--after", messages_after.as_str()];
    let events_args = ["log", "--json", "--steps", "--after", events_after.as_str()];
    let (processes, messages, events) =
        tokio::try_join!(jarvis_json(&["ps", "--json"]), jarvis_json(&messages_args), jarvis_json(&events_args))?;
    Ok(Json(json!({
        "me": me(),
        "build": std::env::var("PAGE_BUILD").unwrap_or_else(|_| "dev".into()),
        "processes": processes,
        "messages": messages,
        "events": events,
    })))
}

async fn send(Json(m): Json<Value>) -> Result<Json<Value>, Failure> {
    let (Some(to), Some(text)) = (m["to"].as_str(), m["text"].as_str()) else {
        return Err(bad("need `to` and `text`"));
    };
    // The text goes in on standard input, so it can contain anything.
    jarvis(&["send", "--as", "human", to, "-"], Some(text), false).await?;
    Ok(Json(json!({"ok": true})))
}

async fn act(Json(m): Json<Value>) -> Result<Json<Value>, Failure> {
    let (Some(action), Some(who)) = (m["action"].as_str(), m["who"].as_str()) else {
        return Err(bad("need `action` and `who`"));
    };
    if !matches!(action, "stop" | "start" | "remove") {
        return Err(bad(format!("`{action}` is not something the page does")));
    }
    // Stopping or removing this process would take the page down with it.
    if who == me() && action != "start" {
        return Err(bad(format!("{who} is the process serving this page. Use `jarvis {action} {who}` in a terminal")));
    }
    let said = jarvis(&[action, who], None, true).await?;
    Ok(Json(json!({"ok": true, "said": said.trim()})))
}

async fn files(Query(q): Params) -> Result<Json<Value>, Failure> {
    let who = q.get("who").ok_or_else(|| bad("need `who`"))?;
    let listed = jarvis(&["show", who], None, false).await?;
    let names: Vec<&str> = listed.lines().filter(|line| !line.is_empty()).collect();
    Ok(Json(json!({ "files": names })))
}

async fn file(Query(q): Params) -> Result<Json<Value>, Failure> {
    let (Some(who), Some(path)) = (q.get("who"), q.get("path")) else {
        return Err(bad("need `who` and `path`"));
    };
    let mut text = jarvis(&["show", who, path], None, false).await?;
    let cut = text.len() > MOST;
    if cut {
        let mut end = MOST;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Ok(Json(json!({ "text": text, "cut": cut })))
}

/// Bind a port, trying for a while: the server this one replaces may be
/// letting go of it at this moment.
async fn bind(port: u16, tries: u32) -> Option<TcpListener> {
    for _ in 0..tries {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
            return Some(listener);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    None
}

async fn listen() -> (TcpListener, u16) {
    if let Some(port) = std::env::var("PORT").ok().and_then(|p| p.parse::<u16>().ok()) {
        let listener = bind(port, 50).await.unwrap_or_else(|| panic!("could not listen on 127.0.0.1:{port} ($PORT)"));
        return (listener, port);
    }
    let own = std::fs::read_to_string(PORT_FILE).ok().and_then(|p| p.trim().parse::<u16>().ok());
    if let Some(port) = own {
        if let Some(listener) = bind(port, 50).await {
            return (listener, port);
        }
        eprintln!("port {port}, which this process normally uses, is taken by something else; using another one for now");
    }
    for port in FIRST_PORT..FIRST_PORT + 200 {
        if let Some(listener) = bind(port, 1).await {
            // The first port this process gets is its own from then on. A
            // port taken only because its own was busy is not remembered.
            if own.is_none() {
                let _ = std::fs::create_dir_all(".build");
                let _ = std::fs::write(PORT_FILE, format!("{port}\n"));
            }
            return (listener, port);
        }
    }
    panic!("no free port from {FIRST_PORT} up");
}

#[tokio::main]
async fn main() {
    let dist = std::env::var("PAGE_DIST").unwrap_or_else(|_| "web/dist".into());
    let app = Router::new()
        .route("/api/snapshot", get(snapshot))
        .route("/api/send", post(send))
        .route("/api/act", post(act))
        .route("/api/files", get(files))
        .route("/api/file", get(file))
        .fallback_service(ServeDir::new(dist));
    let (listener, port) = listen().await;
    println!("page at http://127.0.0.1:{port}");
    axum::serve(listener, app).await.expect("web server stopped");
}
