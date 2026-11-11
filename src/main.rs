//! `jarvis`: one command, used the same way by the human, by agents and by
//! process code.
//!
//! `jarvis up NAME` is a kernel (`kernel.rs`). Every other command is sent,
//! as it was typed, to a kernel on this machine over its socket, and what the
//! kernel answers is printed (`serve.rs` is the other end). Three things are
//! done right here instead: being the far end of a link (`link --stdio`),
//! reading a store that has no kernel (`--store DIR`), and looking after a
//! store's files (`store compact`, `store merge`).
//!
//! The commands and their help are defined in `cli.rs`.

mod children;
mod cli;
mod git;
mod index;
mod kernel;
mod net;
mod serve;
mod store;

use clap::{CommandFactory, FromArgMatches};
use cli::{Cli, Cmd, StoreCmd};
use index::Res;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

fn main() {
    // A reader that stops reading (`jarvis log | head`) ends this program
    // quietly, as it does any other command-line program.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    // `jarvis`, `jarvis help` and `jarvis --help` all print the grouped overview.
    let matches = Cli::command().override_help(cli::overview()).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if cli.command.is_none() {
        print!("{}", cli::overview());
        return;
    }
    match run(cli) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("jarvis: {e}");
            std::process::exit(1);
        }
    }
}

struct ToTerminal;

impl serve::Out for ToTerminal {
    fn line(&mut self, text: &str) -> bool {
        writeln!(std::io::stdout(), "{text}").is_ok()
    }
    fn note(&mut self, text: &str) {
        eprintln!("{text}");
    }
}

fn run(cli: Cli) -> Res<i32> {
    let here = std::env::current_dir()?;
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = cli.command.expect("there is a command");
    let given_store = cli.store.as_deref().map(|dir| here.join(dir));

    match command {
        Cmd::Up(args) => {
            kernel::up(args, cli.store)?;
            return Ok(0);
        }
        Cmd::Link(ref args) if args.stdio => return bridge(&cli.on),
        // Looking after a store's files needs no kernel when the store is named.
        Cmd::Store(ref args) if !matches!(args.what, StoreCmd::Ship) => {
            let named = given_store.clone().or_else(|| match socket(&cli.on) {
                Ok(_) => None,
                Err(_) => std::env::var("JARVIS_STORE").ok().filter(|s| !s.is_empty()).map(PathBuf::from),
            });
            if let Some(dir) = named {
                println!("{}", match &args.what {
                    StoreCmd::Compact { without } => store::compact(&dir, without)?,
                    StoreCmd::Merge { other } => store::merge(&dir, &here.join(other))?,
                    StoreCmd::Ship => unreachable!(),
                });
                return Ok(0);
            }
        }
        _ => {}
    }

    // Reading a store directly: build what its records add up to, and answer from that.
    if let Some(dir) = given_store {
        if !serve::reads_only(&command) {
            return Err("with --store, only `ps`, `messages`, `log`, `kernels` and `show` can be used: they read, and a store alone cannot do more".into());
        }
        if !dir.is_dir() {
            return Err(format!("{} is not a directory", dir.display()).into());
        }
        let conn = index::open_memory()?;
        store::Store::open(&dir, &conn)?.ingest(&conn, None)?;
        let ctx = serve::Ctx::of_store(conn, git::Repo::find(&here).ok());
        // The store was named for this command, not for a kernel: it is not passed on.
        let mut argv = argv;
        if let Some(at) = argv.iter().position(|a| a == "--store") {
            argv.drain(at..(at + 2).min(argv.len()));
        }
        argv.retain(|a| !a.starts_with("--store="));
        return Ok(serve::handle(&ctx, &json!({"argv": argv}), &mut ToTerminal));
    }

    // Everything else is done by a kernel.
    let mut request = json!({
        "argv": argv,
        "by": std::env::var("JARVIS_ID").unwrap_or_default(),
        "cwd": here.display().to_string(),
    });
    match &command {
        Cmd::Send(args) => {
            let text = args.text.join(" ");
            if text == "-" || text.is_empty() {
                if std::io::stdin().is_terminal() {
                    eprintln!("type the message; finish with Ctrl-D on a line of its own");
                }
                let mut body = String::new();
                std::io::stdin().read_to_string(&mut body)?;
                request["stdin"] = json!(body);
            }
        }
        Cmd::Spawn(args) => {
            if let Some(file) = &args.prompt_file {
                let text = std::fs::read_to_string(file).map_err(|e| format!("could not read {file}: {e}"))?;
                request["files"] = json!({ file: text });
            }
        }
        _ => {}
    }
    let mut stream = UnixStream::connect(socket(&cli.on)?)?;
    writeln!(stream, "{request}")?;
    for line in BufReader::new(stream).lines() {
        let Ok(answer) = serde_json::from_str::<Value>(&line?) else { continue };
        if let Some(text) = answer["o"].as_str() {
            println!("{text}");
        } else if let Some(text) = answer["e"].as_str() {
            eprintln!("{text}");
        } else if let Some(code) = answer["end"].as_i64() {
            return Ok(code as i32);
        }
    }
    Err("the kernel stopped before it had answered".into())
}

/// The socket of the kernel that is to carry out a command: the one named
/// with --on if it runs on this machine, else the one this process belongs
/// to (`$JARVIS_KERNEL`), else the only one here.
fn socket(on: &Option<String>) -> Res<PathBuf> {
    let dir = kernel::run_dir();
    let of = |name: &str| dir.join(format!("{name}.sock"));
    let runs = |path: &Path| UnixStream::connect(path).is_ok();
    if let Some(on) = on {
        if runs(&of(on)) {
            return Ok(of(on));
        }
        // Not on this machine: a kernel here passes the command on.
    }
    if let Some(mine) = std::env::var("JARVIS_KERNEL").ok().filter(|k| !k.is_empty()) {
        if runs(&of(&mine)) {
            return Ok(of(&mine));
        }
        return Err(format!("the kernel `{mine}` ($JARVIS_KERNEL) is not running on this machine").into());
    }
    let mut here: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter_map(|n| n.strip_suffix(".sock").map(str::to_string)).collect())
        .unwrap_or_default();
    here.retain(|name| runs(&of(name)));
    here.sort();
    match here.as_slice() {
        [] => Err("no kernel is running on this machine. Start one with `jarvis up NAME` (see `jarvis help up`)".into()),
        [only] => Ok(of(only)),
        many => Err(format!("several kernels run on this machine ({}). Say which with --on NAME, or set $JARVIS_KERNEL", many.join(", ")).into()),
    }
}

/// `jarvis link --stdio`: be the far end of a link made through a command.
/// What arrives on standard input goes to the kernel here, and what the
/// kernel says goes to standard output, until either side ends.
fn bridge(on: &Option<String>) -> Res<i32> {
    let mut stream = UnixStream::connect(socket(on)?)?;
    stream.write_all(b"{\"link\":true}\n")?;
    let mut to_kernel = stream.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin(), &mut to_kernel);
        let _ = to_kernel.shutdown(std::net::Shutdown::Both);
    });
    let mut out = std::io::stdout();
    let mut buffer = [0u8; 65536];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return Ok(0),
            Ok(n) => {
                out.write_all(&buffer[..n])?;
                out.flush()?;
            }
        }
    }
}
