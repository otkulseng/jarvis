//! The server behind the web page. It serves the built page from `web/dist`
//! and answers two questions for it, both by running the `jarvis` command:
//!
//!   GET  /api/snapshot?messages_after=N&events_after=M
//!        who exists, and the messages and facts newer than the given ids
//!   POST /api/send  {"to": ..., "text": ...}
//!        send a message as the human
//!
//! It keeps no state of its own. Listens on 127.0.0.1:$PORT (default 7878).

use axum::extract::Query;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tower_http::services::ServeDir;

type Failure = (StatusCode, String);

/// Run `jarvis` with the given arguments (and optional standard input) and
/// return what it printed.
async fn jarvis(args: &[&str], input: Option<&str>) -> Result<String, Failure> {
    let fail = |text: String| (StatusCode::INTERNAL_SERVER_ERROR, text);
    let mut child = Command::new("jarvis")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| fail(format!("could not run jarvis: {e}")))?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    if let Some(input) = input {
        stdin.write_all(input.as_bytes()).await.map_err(|e| fail(e.to_string()))?;
    }
    drop(stdin);
    let out = child.wait_with_output().await.map_err(|e| fail(e.to_string()))?;
    if !out.status.success() {
        return Err(fail(String::from_utf8_lossy(&out.stderr).trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn jarvis_json(args: &[&str]) -> Result<Value, Failure> {
    let text = jarvis(args, None).await?;
    serde_json::from_str(&text).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("jarvis printed bad JSON: {e}")))
}

async fn snapshot(Query(q): Query<HashMap<String, String>>) -> Result<Json<Value>, Failure> {
    let after = |key: &str| q.get(key).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0).to_string();
    let (messages_after, events_after) = (after("messages_after"), after("events_after"));
    let messages_args = ["messages", "--json", "--after", messages_after.as_str()];
    let events_args = ["log", "--json", "--after", events_after.as_str()];
    let (processes, messages, events) = tokio::try_join!(
        jarvis_json(&["ps", "--json"]),
        jarvis_json(&messages_args),
        jarvis_json(&events_args),
    )?;
    Ok(Json(json!({
        "me": std::env::var("JARVIS_ID").unwrap_or_default(),
        "processes": processes,
        "messages": messages,
        "events": events,
    })))
}

async fn send(Json(m): Json<Value>) -> Result<Json<Value>, Failure> {
    let (Some(to), Some(text)) = (m["to"].as_str(), m["text"].as_str()) else {
        return Err((StatusCode::BAD_REQUEST, "need `to` and `text`".into()));
    };
    // The text goes in on standard input, so it can contain anything.
    jarvis(&["send", "--as", "human", to, "-"], Some(text)).await?;
    Ok(Json(json!({"ok": true})))
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/api/snapshot", get(snapshot))
        .route("/api/send", post(send))
        .fallback_service(ServeDir::new("web/dist"));
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(7878);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap_or_else(|e| panic!("could not listen on 127.0.0.1:{port}: {e}"));
    println!("page at http://127.0.0.1:{port}");
    axum::serve(listener, app).await.expect("web server stopped");
}
