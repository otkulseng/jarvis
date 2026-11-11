//! The server behind the web page. It is the one place that understands
//! jarvis: it follows `jarvis watch`, keeps the current picture (`model.rs`),
//! and tells each open browser what changed in what that browser is showing
//! (`hub.rs`). Browsers only draw.
//!
//!   jarvis watch ──▶ model.rs (what a change means) ──▶ hub.rs (batches) ──▶ browsers
//!
//!   GET  /api/stream?topics=T1,T2    what changes, pushed as it happens
//!        (server-sent events; each event is a JSON list of frames). `map`
//!        and `inbox` are always included. Frames and topics: `model.rs`.
//!   GET  /api/older?topic=T&before=MESSAGE     earlier messages of a topic
//!   GET  /api/steps?who=ID&after=A&before=B    the steps of an earlier turn
//!   POST /api/send   {"to": ..., "text": ...}  a message from the human
//!   POST /api/act    {"action": "stop" | "start" | "remove", "who": ...}
//!   GET  /api/files?who=NAME                   the files of a process
//!   GET  /api/file?who=NAME&path=PATH          one of them, as text
//!   GET, POST /api/positions                   where the reader put the nodes of the map
//!
//! Set by `.jarvis/run`: $PAGE_DIST (the built page, default `web/dist`) and
//! $PAGE_BUILD (the name of this build; an open page reloads itself when it
//! is served by a different one).
//!
//! The port: $PORT if set. Otherwise the one this process used last time
//! (`.build/port`), and the first time, the first free one from 7878 up. So
//! the first page process gets 7878 and keeps it, and others get their own.

mod hub;
mod jarvis;
mod model;
mod threads;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use hub::Hub;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::services::ServeDir;

type Failure = (StatusCode, String);
type Params = Query<HashMap<String, String>>;

const FIRST_PORT: u16 = 7878;
const PORT_FILE: &str = ".build/port";
const POSITIONS_FILE: &str = ".build/positions.json";
/// The most of one file that is sent to the page.
const MOST: usize = 400_000;
/// How often what has changed is sent on to the browsers.
const BATCH_EVERY: Duration = Duration::from_millis(80);

fn fail(text: impl Into<String>) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, text.into())
}

fn bad(text: impl Into<String>) -> Failure {
    (StatusCode::BAD_REQUEST, text.into())
}

fn me() -> String {
    std::env::var("JARVIS_ID").unwrap_or_default()
}

/// Follow jarvis for as long as the server runs.
async fn follow(hub: Arc<Hub>) {
    let (lines, mut changes) = tokio::sync::mpsc::channel(1024);
    tokio::spawn(jarvis::watch(lines));
    while let Some(change) = changes.recv().await {
        // `ready`: the feed has (again) said how things stand. Count from there.
        if change["kind"] == "ready" {
            let pairs = jarvis::list(&["messages", "--all", "--pairs", "--json"]).await;
            let waiting = jarvis::list(&["messages", "--all", "--waiting", "--json", "-n", "100000"]).await;
            match (pairs, waiting) {
                (Ok(pairs), Ok(waiting)) => hub.load(&pairs, &waiting),
                (Err(e), _) | (_, Err(e)) => eprintln!("could not read the messages: {e}"),
            }
        }
        hub.apply(&change);
    }
}

async fn stream(State(hub): State<Arc<Hub>>, Query(q): Params) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let asked: Vec<String> = q.get("topics").map(|t| t.split(',').filter(|t| !t.is_empty()).map(String::from).collect()).unwrap_or_default();
    let mut topics: HashSet<String> = asked.iter().cloned().collect();
    topics.extend(["map".to_string(), "inbox".to_string()]);
    let hello = json!({
        "type": "hello",
        "me": me(),
        "build": std::env::var("PAGE_BUILD").unwrap_or_else(|_| "dev".into()),
    });
    let (first, sender, receiver) = hub.connect(topics.clone(), hello);
    // The history of what it opened follows, as soon as jarvis has given it.
    tokio::spawn(async move {
        for topic in topics.iter().filter(|topic| *topic != "map") {
            let thread = threads::open(topic).await;
            if sender.send(json!([thread]).to_string()).await.is_err() {
                return;
            }
        }
    });
    let batches = tokio_stream::once(first).chain(ReceiverStream::new(receiver));
    Sse::new(batches.map(|batch| Ok(Event::default().data(batch)))).keep_alive(KeepAlive::default())
}

async fn older(Query(q): Params) -> Result<Json<Value>, Failure> {
    let topic = q.get("topic").ok_or_else(|| bad("need `topic`"))?;
    let before = q.get("before").map(String::as_str).filter(|b| !b.is_empty());
    let messages = threads::messages(topic, before).await.map_err(fail)?;
    Ok(Json(json!({"more": messages.len() >= threads::PAGE, "messages": messages})))
}

async fn steps(Query(q): Params) -> Result<Json<Value>, Failure> {
    let who = q.get("who").ok_or_else(|| bad("need `who`"))?;
    let number = |key: &str| q.get(key).and_then(|v| v.parse::<i64>().ok());
    let steps = threads::steps(who, number("after").unwrap_or(0), number("before")).await.map_err(fail)?;
    Ok(Json(json!({ "steps": steps })))
}

async fn positions() -> Json<Value> {
    let saved = tokio::fs::read_to_string(POSITIONS_FILE).await.ok().and_then(|text| serde_json::from_str(&text).ok());
    Json(saved.unwrap_or_else(|| json!({})))
}

async fn save_positions(Json(positions): Json<Value>) -> Result<Json<Value>, Failure> {
    if !positions.is_object() {
        return Err(bad("need an object of positions"));
    }
    let _ = tokio::fs::create_dir_all(".build").await;
    tokio::fs::write(POSITIONS_FILE, positions.to_string()).await.map_err(|e| fail(e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

async fn send(Json(m): Json<Value>) -> Result<Json<Value>, Failure> {
    let (Some(to), Some(text)) = (m["to"].as_str(), m["text"].as_str()) else {
        return Err(bad("need `to` and `text`"));
    };
    // The text goes in on standard input, so it can contain anything.
    // The human, at the kernel this page is on.
    let human = format!("human@{}", me().split('@').nth(1).unwrap_or(""));
    jarvis::run(&["send", "--as", &human, to, "-"], Some(text), false).await.map_err(fail)?;
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
    let said = jarvis::run(&[action, who], None, true).await.map_err(fail)?;
    Ok(Json(json!({"ok": true, "said": said.trim()})))
}

async fn files(Query(q): Params) -> Result<Json<Value>, Failure> {
    let who = q.get("who").ok_or_else(|| bad("need `who`"))?;
    let listed = jarvis::run(&["show", who], None, false).await.map_err(fail)?;
    let names: Vec<&str> = listed.lines().filter(|line| !line.is_empty()).collect();
    Ok(Json(json!({ "files": names })))
}

async fn file(Query(q): Params) -> Result<Json<Value>, Failure> {
    let (Some(who), Some(path)) = (q.get("who"), q.get("path")) else {
        return Err(bad("need `who` and `path`"));
    };
    let mut text = jarvis::run(&["show", who, path], None, false).await.map_err(fail)?;
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
    let hub = Arc::new(Hub::default());
    tokio::spawn(follow(hub.clone()));
    let ticking = hub.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(BATCH_EVERY);
        loop {
            tick.tick().await;
            ticking.flush();
        }
    });
    let app = Router::new()
        .route("/api/stream", get(stream))
        .route("/api/older", get(older))
        .route("/api/steps", get(steps))
        .route("/api/send", post(send))
        .route("/api/act", post(act))
        .route("/api/files", get(files))
        .route("/api/file", get(file))
        .route("/api/positions", get(positions).post(save_positions))
        .with_state(hub)
        .fallback_service(ServeDir::new(dist));
    let (listener, port) = listen().await;
    println!("page at http://127.0.0.1:{port}");
    axum::serve(listener, app).await.expect("web server stopped");
}
