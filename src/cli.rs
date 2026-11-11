//! The command line: every command, its arguments, and its help.
//!
//! The help is written here, next to the arguments it describes, and is the
//! one description of how to use jarvis. People read it with `jarvis help`;
//! agents are shown the same text. Each command is a struct whose fields are
//! its arguments (this is clap's "derive" style): the `///` comment on a
//! field is that argument's help.

use clap::{Args, CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "jarvis", version, about = "Many long-lived processes, each with its own code and agent, working together for one human.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

/// The commands in the order they are listed, under these headings.
const GROUPS: &[(&str, &[&str])] = &[
    ("Starting", &["init", "up"]),
    ("Processes", &["spawn", "ps", "tag", "stop", "start", "remove"]),
    ("Talking", &["send", "messages"]),
    ("Looking", &["show", "log", "watch"]),
];

/// What `jarvis`, `jarvis help` and `jarvis --help` print: every command with
/// its one-line summary, grouped. The summaries are taken from the commands
/// themselves, so this cannot say something different from their own help.
pub fn overview() -> String {
    let command = Cli::command();
    let mut text = String::from(
        "jarvis: one system of many long-lived processes. Each process is an expert\n\
         with a name, a mailbox, its own directory of code and notes (a git branch),\n\
         and an agent that can change them. They work together, by sending each\n\
         other messages, to get the best results for one human.\n",
    );
    for (heading, names) in GROUPS {
        text.push_str(&format!("\n{heading}\n"));
        for name in *names {
            let sub = command.find_subcommand(name).expect("every listed command exists");
            let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
            text.push_str(&format!("  {name:<9} {about}\n"));
        }
    }
    text.push_str(
        "\nWHO is a process's name. `human` is the person.\n\
         Commands that print a list also take --json, for programs.\n\
         \n\
         jarvis help <command>    everything about one command, with examples\n",
    );
    text
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Make the git repository here a jarvis home
    #[command(long_about = "Make the git repository here a jarvis home.\n\n\
        Run once, anywhere inside a git repository. It creates the `.jarvis/` directory \
        at the top of the repository, which holds the registry (who exists, the \
        messages, what happened) and, later, the directory of each process. Nothing \
        is started. Running it again changes nothing.\n\n\
        A home is one repository with one kernel. Every other command finds the home \
        by looking upwards from the current directory, or uses $JARVIS_HOME.")]
    #[command(after_long_help = "Examples:\n  jarvis init\n  jarvis up")]
    Init,

    /// Run the kernel: the one program that makes the processes go
    #[command(long_about = "Run the kernel: the one program that makes the processes go.\n\n\
        Leave it running in a terminal. While it runs, it:\n\
        \x20 - gives each process's waiting messages to that process (its handler, then its agent);\n\
        \x20 - keeps each process's service running;\n\
        \x20 - commits what a process changed in its directory after each turn;\n\
        \x20 - records everything that happens (see `jarvis log`).\n\n\
        Without it nothing happens: messages wait in the mailboxes until it is started. \
        Only one kernel can run per home.\n\n\
        Ctrl-C stops it, together with every service and every turn in progress. \
        Nothing is lost: messages that were being worked on are given again, marked \
        as such, the next time it runs. The same holds if it is killed or the machine \
        goes down.")]
    #[command(after_long_help = "Examples:\n  jarvis up\n  jarvis up 2> kernel.log     # what happens is printed on standard error")]
    Up,

    /// Create a process: a new expert with its own name, directory and agent
    #[command(long_about = "Create a process: a new expert with its own name, directory and agent.\n\n\
        The name is the process: its address for messages, and its git branch, \
        `jarvis/NAME`, which is checked out as its own directory. No two processes \
        share a name. The new process is a peer of every other process. Anyone can \
        spawn: the human, or a process during its turn.\n\n\
        What it starts with is your choice:\n\
        \x20 (no options)   Nothing. It does nothing, and costs nothing, until someone\n\
        \x20                sends it a message; that message tells it what to be.\n\
        \x20 --prompt       Who it is and what it is for.\n\
        \x20 --from WHAT    The files and history of a branch: an expert's (`page`),\n\
        \x20                a process's (`jarvis/NAME`), or any tag or commit.\n\
        \x20 --tree DIR     A copy of a directory.\n\n\
        Unless it starts with nothing, it is woken at once by a first message from \
        you. Prints the name of the new process.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis spawn scratch\n\
        \x20     A blank process, waiting for its first message.\n\n\
        \x20 jarvis spawn polaris --prompt \"You are our expert on the Polaris cluster.\"\n\
        \x20     An expert that starts from a prompt alone.\n\n\
        \x20 jarvis spawn page0 --from page\n\
        \x20     One more process of the expert `page`: the branch `page` is the code and\n\
        \x20     knowledge all of them share, and `jarvis/page0` is where this one works.\n\
        \x20     Run it several times, with different names, for several processes on one code.\n\n\
        \x20 jarvis spawn gpu --from jarvis/lammps1 --prompt \"You work on the GPU kernels.\"\n\
        \x20     A fork of the process `lammps1`: what it has now, landed or not.\n\n\
        \x20 jarvis spawn sim --tree ~/code/sim --prompt-file brief.md\n\
        \x20     Bring in files that are not in this repository yet.")]
    Spawn(Spawn),

    /// Who exists and what each is doing; or everything about one process
    #[command(long_about = "Who exists and what each is doing; or everything about one process.\n\n\
        Without WHO: one line per process, with its name, state, who made it, and \
        how it describes itself (the first line of its .jarvis/ABOUT.md). The state is:\n\
        \x20 idle           waiting for a message\n\
        \x20 thinking 3m    a turn is in progress, for this long\n\
        \x20 failing x4     its last 4 turns failed (an outage, a broken harness); its\n\
        \x20                mail is kept and the kernel keeps trying\n\
        \x20 stopped        someone ran `jarvis stop` on it; its mail waits\n\n\
        With WHO: everything about that process: its branch and directory, who made \
        it, its service, what it has cost, how much mail is waiting, and how its last \
        turn went, with the whole error if it failed.\n\n\
        If the kernel is not running, that is said on standard error.")]
    #[command(after_long_help = "Examples:\n  jarvis ps\n  jarvis ps lammps\n  jarvis ps --json")]
    Ps(Ps),

    /// End what a process is doing now; its mail waits
    #[command(long_about = "End what a process is doing now; its mail waits.\n\n\
        The turn in progress, if any, is ended at once, together with the process's \
        service and everything either of them started. Nothing is deleted: its \
        directory, branch and mail stay as they are. Messages sent to it meanwhile \
        wait. `jarvis start` lets it go on; the messages of the turn that was cut \
        short are then given to it again, marked as such.")]
    #[command(after_long_help = "Examples:\n  jarvis stop lammps\n  jarvis stop lammps lammps-gpu polaris")]
    Stop(Who),

    /// Let a stopped process go on
    #[command(long_about = "Let a stopped process go on.\n\n\
        Its service is started again and its waiting mail is given to it. On a process \
        that is `failing`, stop followed by start makes the kernel try again at once \
        instead of after its pause.")]
    #[command(after_long_help = "Examples:\n  jarvis start lammps")]
    Start(Who),

    /// Delete a process for good: its directory, branch, mail and history
    #[command(long_about = "Delete a process for good: its directory, branch, mail and history.\n\n\
        What it is doing is ended first, as with `jarvis stop`. Then its directory, \
        its branch `jarvis/NAME`, the messages to and from it, and the facts recorded \
        about it are deleted. One fact is kept: that it was removed, by whom, what it \
        had cost, and the commit its branch was at. Afterwards the name is free to be \
        used again.\n\n\
        Work it landed on an expert's branch is not touched. Work it had not landed \
        goes with its branch: the commit is printed, and \
        `git branch jarvis/NAME <commit>` brings the files back for as long as git \
        still has them (weeks, by default). With --keep-branch the branch stays, and \
        the name stays taken.\n\n\
        Branches on a remote are never touched. A process cannot remove itself.")]
    #[command(after_long_help = "Examples:\n  jarvis remove scratch\n  jarvis remove page0 page1\n  jarvis remove lammps-gpu --keep-branch")]
    Remove(Remove),

    /// Say what a process belongs to: a project, a place, anything
    #[command(long_about = "Say what a process belongs to: a project, a place, anything.\n\n\
        A tag is a name with a value, such as `project=moe` or `place=cluster one`. \
        A process has at most one value for each name. Tags are free-form: jarvis \
        stores and reports them (`jarvis ps`, `jarvis watch`) and gives them no \
        meaning. They are for people, processes and programs to find and group \
        processes by; the page arranges its map by them.\n\n\
        Anyone may tag any process. `NAME=` with nothing after it removes that tag. \
        Without any tags given, the process's tags are printed.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis tag lammps1 project=moe\n\
        \x20 jarvis tag lammps1 \"place=cluster one\" role=lead\n\
        \x20 jarvis tag lammps1 role=            remove the tag `role`\n\
        \x20 jarvis tag lammps1                  print its tags")]
    Tag(Tag),

    /// Send a message to a process, or to the human
    #[command(long_about = "Send a message to a process, or to the human.\n\n\
        The message goes into the recipient's mailbox and starts a turn there as soon \
        as the recipient is free. This command does not wait for an answer: an answer, \
        if one comes, is a new message to you (see `jarvis messages`).\n\n\
        Sending is the only way to tell anyone anything. What an agent writes at the \
        end of its turn is logged and reaches nobody.\n\n\
        Everything after TO is the text. With no text, or with `-`, the text is read \
        from standard input, which is the way to send several lines or anything with \
        quotes in it. A message is never dropped: if the recipient cannot take it now \
        (stopped, failing, or no kernel running), it waits, and you are told.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis send polaris How do I get an interactive GPU node?\n\n\
        \x20 jarvis send human - <<'EOF'\n\
        \x20 The build works on Polaris now.\n\
        \x20 Two tests still fail; details are in my notes.\n\
        \x20 EOF\n\n\
        \x20 git diff | jarvis send lammps")]
    Send(SendArgs),

    /// Read messages: all of them, or those to and from one process
    #[command(long_about = "Read messages: all of them, or those to and from one process.\n\n\
        Oldest first, ending with the newest. Each shows its number, the time, who sent \
        it to whom, and, if it has not been dealt with yet, that it is waiting. Use \
        `human` as WHO for what was said to and by the person.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis messages                 the last 20 messages between anyone\n\
        \x20 jarvis messages polaris -n 5    the last 5 to or from polaris\n\
        \x20 jarvis messages --waiting       what has not been dealt with yet\n\
        \x20 jarvis messages --after 120     everything newer than message 120")]
    Messages(Messages),

    /// Look at the files of a process: its code and its notes
    #[command(long_about = "Look at the files of a process: its code and its notes.\n\n\
        Without PATH: the names of all files on the process's branch. With PATH: the \
        content of that file. This shows what is committed, which is everything up to \
        the end of the process's last turn. Reading is always allowed and disturbs \
        nobody; a process's notes are in its `.jarvis/` directory.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis show polaris\n\
        \x20 jarvis show polaris .jarvis/ABOUT.md\n\
        \x20 jarvis show lammps src/pair_lj.cpp")]
    Show(Show),

    /// What happened: every turn, message, commit and failure, in order
    #[command(long_about = "What happened: every turn, message, commit and failure, in order.\n\n\
        The kernel records each thing it does or sees as a fact: a message sent, a \
        turn started, finished (with what it cost) or failed (with the error), a \
        commit, a service starting or exiting, a process being made, stopped or \
        started. This prints them, oldest first. With WHO, only what that process \
        did and the messages sent to it.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis log                 the last 30 facts\n\
        \x20 jarvis log -f              keep printing as things happen (Ctrl-C to leave)\n\
        \x20 jarvis log -fs page        watch what page's agent is doing, step by step\n\
        \x20 jarvis log lammps -n 100   the last 100 about lammps\n\
        \x20 jarvis log --json          for programs: every field of every fact")]
    Log(Log),

    /// Follow every change as it happens, as JSON lines, for programs
    #[command(long_about = "Follow every change as it happens, as JSON lines, for programs.\n\n\
        Prints one JSON object per line and keeps running. It is how a program (a \
        page, a monitor, a process's own code) stays up to date without asking again \
        and again. The lines:\n\
        \x20 {\"kind\":\"process\",\"process\":{...}}   a process as `jarvis ps --json` shows it:\n\
        \x20                                         every process at the start, then each one\n\
        \x20                                         whenever something about it changes\n\
        \x20 {\"kind\":\"gone\",\"name\":NAME}           a process was removed\n\
        \x20 {\"kind\":\"ready\",\"cursor\":N}           the start is complete; facts after N follow\n\
        \x20 {\"kind\":\"fact\",\"fact\":{...}}         a fact as `jarvis log --json` shows it. A\n\
        \x20                                         `send` fact also has `message`: the message\n\
        \x20 {\"kind\":\"message\",\"message\":{...}}   a message whose status changed (it was\n\
        \x20                                         taken up, dealt with, or put back)\n\n\
        Facts come in order, each exactly once. Without --after, only facts from now \
        on; with it, also the earlier ones above that number. A program that is cut \
        off starts again with --after set to the last fact it saw.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis watch                  everything from now on\n\
        \x20 jarvis watch --steps          also each step agents take\n\
        \x20 jarvis watch --after 5120     continue after fact 5120")]
    Watch(Watch),
}

#[derive(Args)]
pub struct Spawn {
    /// What to call it. Unique; letters, digits, `-`, `_` and `.` only
    pub name: String,

    /// Who it is and what it is for. Saved as .jarvis/PROMPT.md in its directory
    #[arg(long, value_name = "TEXT", conflicts_with = "prompt_file")]
    pub prompt: Option<String>,

    /// The same, read from a file
    #[arg(long, value_name = "FILE")]
    pub prompt_file: Option<String>,

    /// Start from a git branch (an expert's, or a process's), tag or commit
    #[arg(long, value_name = "WHAT", conflicts_with = "tree",
          long_help = "Start from a git branch (an expert's, or a process's), tag or commit.\n\n\
              The new process gets the same files and the same history on a branch of its own, \
              so work can later be merged in either direction. Everything comes along, the \
              `.jarvis/` directory included (prompt, notes, service, tools), but no \
              conversation: it starts with a fresh memory.\n\n\
              An expert is a branch with a plain name, such as `page`, that its processes \
              land their finished work on. A process's own branch is `jarvis/NAME`. WHAT is \
              tried as something git knows first, then as a process's name, then as a branch \
              of that name on the remote (`origin/WHAT`).")]
    pub from: Option<String>,

    /// Start from a copy of a directory of files
    #[arg(long, value_name = "DIR",
          long_help = "Start from a copy of a directory of files.\n\n\
              Everything in DIR is copied, hidden files included, and becomes the first \
              commit of the new process. Files a `.gitignore` in DIR excludes are copied but \
              not committed. DIR must not be the top of a git repository; for code that git \
              already has, use --from.")]
    pub tree: Option<String>,

    /// Which agent harness runs it: the name of a file in `harness/`
    #[arg(long, value_name = "NAME", default_value = "claude")]
    pub harness: String,

    /// A tag for it, such as `project=moe` (see `jarvis help tag`). Can be given several times
    #[arg(long = "tag", value_name = "NAME=VALUE")]
    pub tags: Vec<String>,
}

#[derive(Args)]
pub struct Tag {
    /// The process, by name
    pub who: String,

    /// Tags to set, as NAME=VALUE; NAME= removes one
    #[arg(value_name = "NAME=VALUE")]
    pub tags: Vec<String>,
}

#[derive(Args)]
pub struct Ps {
    /// Show everything about this one process
    pub who: Option<String>,

    /// Print JSON instead of a table
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct Who {
    /// One or more processes, by name
    #[arg(required = true)]
    pub who: Vec<String>,
}

#[derive(Args)]
pub struct SendArgs {
    /// Who it is for: a process's name, or `human`
    pub to: String,

    /// The message. With none, or `-`, it is read from standard input
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub text: Vec<String>,

    /// Send as someone else, such as `human`. For programs that pass on what a person typed
    #[arg(long = "as", value_name = "WHO")]
    pub sender: Option<String>,
}

#[derive(Args)]
pub struct Remove {
    /// One or more processes, by name
    #[arg(required = true, value_name = "WHO")]
    pub who: Vec<String>,

    /// Keep its git branch, so that its files and history stay (the name stays taken)
    #[arg(long)]
    pub keep_branch: bool,
}

#[derive(Args)]
pub struct Messages {
    /// Only messages to or from this process (or `human`)
    pub who: Option<String>,

    /// Only messages that have not been dealt with yet
    #[arg(long)]
    pub waiting: bool,

    /// Only messages between WHO and this other process (or `human`)
    #[arg(long, value_name = "OTHER", requires = "who")]
    pub with: Option<String>,

    /// Only messages with a number above this
    #[arg(long, value_name = "NUMBER", default_value_t = 0)]
    pub after: i64,

    /// Only messages with a number below this (to read further back)
    #[arg(long, value_name = "NUMBER")]
    pub before: Option<i64>,

    /// Instead of messages: for each two who have written to each other, how
    /// many messages, how many of them waiting, how many written by jarvis on
    /// someone's behalf, and when the last one was
    #[arg(long, conflicts_with_all = ["who", "waiting"])]
    pub pairs: bool,

    /// How many, counted from the newest [default: 20, or 500 with --json]
    #[arg(short = 'n', value_name = "COUNT")]
    pub count: Option<i64>,

    /// Print JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct Watch {
    /// Also print the facts above this number that are already there
    #[arg(long, value_name = "NUMBER")]
    pub after: Option<i64>,

    /// Also print each step agents take inside their turns
    #[arg(short = 's', long)]
    pub steps: bool,
}

#[derive(Args)]
pub struct Show {
    /// The process, by name
    pub who: String,

    /// A file in its directory. Without it, the names of all its files
    pub path: Option<String>,
}

#[derive(Args)]
pub struct Log {
    /// Only what this process did, and the messages sent to it
    pub who: Option<String>,

    /// Only facts with a number above this
    #[arg(long, value_name = "NUMBER")]
    pub after: Option<i64>,

    /// Only facts with a number below this
    #[arg(long, value_name = "NUMBER")]
    pub before: Option<i64>,

    /// How many, counted from the newest [default: 30, or 500 with --json]
    #[arg(short = 'n', value_name = "COUNT")]
    pub count: Option<i64>,

    /// Keep running and print new facts as they happen
    #[arg(short = 'f', long)]
    pub follow: bool,

    /// Also show each step agents take inside their turns: what they say,
    /// the tools they use, and what comes back
    #[arg(short = 's', long)]
    pub steps: bool,

    /// Print JSON instead of text
    #[arg(long)]
    pub json: bool,
}
