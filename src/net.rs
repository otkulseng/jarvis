//! Links: the connections between kernels.
//!
//! A link is two streams of JSON lines, one each way. What carries them does
//! not matter to the kernel: a local socket to a kernel on the same machine,
//! or the input and output of any command (`ssh host jarvis link --stdio`).
//! This file makes the connection and turns it into events for the kernel;
//! what the lines mean is decided in `kernel.rs`.

use crate::children;
use crate::index::Res;
use crate::kernel::{Config, Event};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;

/// Ends a link from this side.
pub type Close = Box<dyn Fn() + Send>;

/// Lines given to the returned sender are written out, in order, by a thread
/// of their own, so that a slow link never holds the kernel up.
fn writer(mut out: impl Write + Send + 'static) -> Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in rx {
            if out.write_all(line.as_bytes()).is_err() || out.write_all(b"\n").is_err() || out.flush().is_err() {
                break;
            }
        }
    });
    tx
}

/// Hand every line that arrives to the kernel. Returns why it ended.
fn pump(id: u64, reader: impl BufRead, tx: &Sender<Event>) -> String {
    for line in reader.lines() {
        match line {
            Ok(line) => {
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                if tx.send(Event::LinkLine { id, value }).is_err() {
                    return "the kernel is stopping".into();
                }
            }
            Err(e) => return e.to_string(),
        }
    }
    "the other end closed it".into()
}

/// Another kernel (or a `jarvis link --stdio`) connected to this kernel's
/// socket and asked to be a link. Runs until the link ends.
pub fn accept(stream: UnixStream, reader: BufReader<UnixStream>, id: u64, tx: &Sender<Event>) {
    let (Ok(out), Ok(closer)) = (stream.try_clone(), stream.try_clone()) else { return };
    let close: Close = Box::new(move || {
        let _ = closer.shutdown(std::net::Shutdown::Both);
    });
    if tx.send(Event::LinkOpen { id, tx: writer(out), close }).is_err() {
        return;
    }
    let why = pump(id, reader, tx);
    let _ = tx.send(Event::LinkDown { id, why });
}

/// Make the link `spec` describes, in the background. The kernel hears
/// `LinkOpen` if it comes up, and `LinkDown` when it ends or could not be made.
pub fn dial(spec: String, id: u64, cfg: Arc<Config>, tx: Sender<Event>) {
    thread::spawn(move || {
        let why = connect(&spec, id, &cfg, &tx).unwrap_or_else(|e| e.to_string());
        let _ = tx.send(Event::LinkDown { id, why });
    });
}

fn connect(spec: &str, id: u64, cfg: &Config, tx: &Sender<Event>) -> Res<String> {
    let v: Value = serde_json::from_str(spec)?;
    if let Some(kernel) = v["kernel"].as_str() {
        let mut stream = UnixStream::connect(cfg.run_dir.join(format!("{kernel}.sock")))
            .map_err(|_| format!("no kernel called `{kernel}` is running on this machine (its socket would be in {})", cfg.run_dir.display()))?;
        stream.write_all(b"{\"link\":true}\n")?;
        let (out, closer) = (stream.try_clone()?, stream.try_clone()?);
        let close: Close = Box::new(move || {
            let _ = closer.shutdown(std::net::Shutdown::Both);
        });
        tx.send(Event::LinkOpen { id, tx: writer(out), close }).map_err(|_| "the kernel is stopping")?;
        return Ok(pump(id, BufReader::new(stream), tx));
    }
    let words: Vec<String> = v["command"].as_array().map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let Some((program, args)) = words.split_first() else { return Err("the link names neither a kernel nor a command".into()) };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("could not run `{program}`: {e}"))?;
    let pid = child.id();
    // What the command prints as errors (ssh's complaints, say) is the reason
    // when the link fails, so its last lines are kept.
    let errors = child.stderr.take().expect("stderr is piped");
    let said = thread::spawn(move || {
        let mut last: Vec<String> = Vec::new();
        for line in BufReader::new(errors).lines().map_while(Result::ok) {
            if !line.trim().is_empty() {
                last.push(line.trim_end().to_string());
                if last.len() > 8 {
                    last.remove(0);
                }
            }
        }
        last.join(" / ")
    });
    let close: Close = Box::new(move || children::signal(pid, libc::SIGKILL));
    tx.send(Event::LinkOpen { id, tx: writer(child.stdin.take().expect("stdin is piped")), close }).map_err(|_| "the kernel is stopping")?;
    let why = pump(id, BufReader::new(child.stdout.take().expect("stdout is piped")), tx);
    children::signal(pid, libc::SIGKILL);
    let status = child.wait().map(|s| s.to_string()).unwrap_or_default();
    let said = said.join().unwrap_or_default();
    Ok(if said.is_empty() {
        format!("{why}; the command ended with {status} and printed no error")
    } else {
        format!("the command said: {said} ({status})")
    })
}
