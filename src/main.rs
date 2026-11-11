//! `jarvis`: one command, used the same way by the human, by agents and by
//! process code.
//!
//! `jarvis up NAME` is a kernel (`kernel/`). Every other command is sent, as
//! it was typed, to a kernel on this machine over its socket, and what the
//! kernel answers is printed (`serve.rs` is the other end). One thing is done
//! right here instead: being the far end of a link (`link --stdio`).
//!
//! The commands and their help are defined in `cli.rs`.

mod children;
mod cli;
mod git;
mod index;
mod kernel;
mod net;
mod serve;

use clap::{CommandFactory, FromArgMatches};
use cli::{Cli, Cmd};
use index::Res;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

fn main() {
    // A reader that stops reading (`jarvis log | head`) ends this program
    // quietly, as it does any other command-line program. (A kernel asks for
    // the opposite when it starts: see `kernel::up`.)
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

fn run(cli: Cli) -> Res<i32> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let command = cli.command.expect("there is a command");
    match &command {
        Cmd::Up(_) => {
            let Cmd::Up(args) = command else { unreachable!() };
            kernel::up(args)?;
            return Ok(0);
        }
        Cmd::Link(args) if args.stdio => return bridge(args.kernel.as_deref()),
        _ => {}
    }

    // Everything else is done by a kernel.
    let mut request = json!({"argv": argv, "by": std::env::var("JARVIS_ID").unwrap_or_default()});
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
    let mut stream = UnixStream::connect(socket(None)?)?;
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
    Err("the kernel stopped before it had answered. Start it again with `jarvis up NAME`; what it was doing is carried on".into())
}

/// The socket of the kernel a command is given to: the one named, else the
/// one this process belongs to or that $JARVIS_KERNEL names, else the only
/// kernel running on this machine. A kernel that is named must be running
/// here: nothing is ever handed to another one instead.
fn socket(named: Option<&str>) -> Res<PathBuf> {
    let dir = kernel::run_dir();
    let of = |name: &str| dir.join(format!("{name}.sock"));
    let runs = |path: &Path| UnixStream::connect(path).is_ok();
    let from_env = std::env::var("JARVIS_KERNEL").ok().filter(|k| !k.is_empty());
    if let Some(name) = named.map(str::to_string).or(from_env.clone()) {
        if runs(&of(&name)) {
            return Ok(of(&name));
        }
        let how = if named.is_some() { String::new() } else { " ($JARVIS_KERNEL)".to_string() };
        return Err(format!(
            "the kernel `{name}`{how} is not running on this machine: nothing answers at {}. Start it with `jarvis up {name}`; \
             if it runs with another $JARVIS_RUN, set the same here",
            of(&name).display()
        )
        .into());
    }
    let mut here: Vec<String> = std::fs::read_dir(&dir)
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter_map(|n| n.strip_suffix(".sock").map(str::to_string)).collect())
        .unwrap_or_default();
    here.retain(|name| runs(&of(name)));
    here.sort();
    match here.as_slice() {
        [] => Err(format!("no kernel is running on this machine (none answers in {}). Start one with `jarvis up NAME` (see `jarvis help up`)", dir.display()).into()),
        [only] => Ok(of(only)),
        many => Err(format!("several kernels run on this machine ({}). Say which one you are on: set $JARVIS_KERNEL to its name", many.join(", ")).into()),
    }
}

/// `jarvis link --stdio [NAME]`: be the far end of a link made through a
/// command. What arrives on standard input goes to the kernel here, and what
/// the kernel says goes to standard output, until either side ends.
fn bridge(named: Option<&str>) -> Res<i32> {
    let mut stream = UnixStream::connect(socket(named)?)?;
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
