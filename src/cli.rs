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

    /// Have this kernel do it: one on this machine, or any that can be reached over links
    #[arg(long, global = true, value_name = "KERNEL")]
    pub on: Option<String>,

    /// Where the records are kept. With `up`: the store this kernel writes to.
    /// With `ps`, `messages`, `log`, `kernels`: read this store directly, with no kernel running
    #[arg(long, global = true, value_name = "DIR")]
    pub store: Option<String>,
}

/// The commands in the order they are listed, under these headings.
const GROUPS: &[(&str, &[&str])] = &[
    ("Kernels", &["up", "kernels", "link"]),
    ("Processes", &["spawn", "ps", "tag", "stop", "start", "remove"]),
    ("Talking", &["send", "messages"]),
    ("Looking", &["show", "log", "watch"]),
    ("Records", &["store"]),
];

/// What `jarvis`, `jarvis help` and `jarvis --help` print: every command with
/// its one-line summary, grouped. The summaries are taken from the commands
/// themselves, so this cannot say something different from their own help.
pub fn overview() -> String {
    let command = Cli::command();
    let mut text = String::from(
        "jarvis: one system of many long-lived processes. Each process has a name, its\n\
         own directory of code and notes (a git branch), and an agent that can change\n\
         them. Processes are hosted by kernels, which may run on many machines and\n\
         pass messages between each other. Processes work together, by sending each\n\
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
        "\nWHO is a process: `lammps` (the one on this kernel, never another), or\n\
         `lammps@cannon-a` (that one exactly, wherever it is). `human` is the person.\n\
         \n\
         Every command is carried out by a kernel: the one you are a process of, or the\n\
         only one on this machine, or the one named with --on KERNEL or $JARVIS_KERNEL.\n\
         Commands that print a list also take --json, for programs.\n\
         \n\
         jarvis help <command>    everything about one command, with examples\n",
    );
    text
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Run a kernel: the program that hosts processes and passes messages on
    #[command(long_about = "Run a kernel: the program that hosts processes and passes messages on.\n\n\
        Leave it running in a terminal. A kernel:\n\
        \x20 - hosts processes: gives each its waiting messages (its handler, then its agent),\n\
        \x20   keeps its service running, and commits what it changed after each turn;\n\
        \x20 - carries out the `jarvis` commands given to it;\n\
        \x20 - passes messages to and from the kernels it is linked to (see `jarvis help link`);\n\
        \x20 - writes down everything that happens, and puts that in the store.\n\n\
        NAME is the kernel's name, and you choose it. It is the second half of the \
        address of every process it hosts (`lammps@NAME`) and of their branches, so \
        it must be different from every kernel this one will ever be linked to or \
        share a repository or store with, also ones that are gone. Starting a kernel \
        again under its name carries on where it stopped. A name whose kernel lost \
        its own directory cannot be taken again: choose a new one.\n\n\
        Three places matter, and many kernels can share the first two:\n\
        \x20 the repository   all code, as git branches. The one you are in, or --repo.\n\
        \x20 the store        all records. --store, or $JARVIS_STORE, or `.jarvis/store`\n\
        \x20                  in the repository.\n\
        \x20 its own state    its database and its processes' working directories. --state,\n\
        \x20                  or `.jarvis/kernel/NAME` in the repository. On a cluster, put\n\
        \x20                  the first two on network storage and this on the node's own disk.\n\n\
        Ctrl-C stops it, together with every service and every turn in progress. \
        Nothing is lost: messages that were being worked on are given again, marked \
        as such, the next time it runs. The same holds if it is killed.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis up laptop\n\
        \x20 jarvis up cannon-4471 --repo /n/proj/jarvis --store /n/proj/jarvis-store --state /scratch/$USER/jarvis\n\
        \x20 jarvis up laptop 2> kernel.log     # what happens is printed on standard error")]
    Up(Up),

    /// The kernels that are known, and whether each can be reached
    #[command(long_about = "The kernels that are known, and whether each can be reached.\n\n\
        One line per kernel: this one, those reachable over links (and through which \
        neighbour, how many hops away), and those only known from their records. For \
        each: when it last wrote a record, and whether it said it closed.\n\n\
        A kernel that was killed never says so: it just stops writing. Whether it is \
        gone is for you to judge from what is shown. --close NAME writes down that \
        you take it to be gone: its processes are then no longer listed and mail to \
        it is refused. Should it write again, it counts as running again.")]
    #[command(after_long_help = "Examples:\n  jarvis kernels\n  jarvis kernels --close cannon-4471")]
    Kernels(Kernels),

    /// Connect this kernel to another, so that messages can pass between them
    #[command(long_about = "Connect this kernel to another, so that messages can pass between them.\n\n\
        A link carries messages and commands both ways. Kernels pass on what is not \
        for them, so two kernels need not be linked directly: a path of links is enough.\n\n\
        \x20 jarvis link                    list this kernel's links\n\
        \x20 jarvis link NAME               link to the kernel NAME on this machine\n\
        \x20 jarvis link -- COMMAND ...     link through a command: this kernel runs it, and\n\
        \x20                                talks to the other kernel through its input and\n\
        \x20                                output. The command must end up running\n\
        \x20                                `jarvis link --stdio` where the other kernel is.\n\
        \x20 jarvis link --remove ...       forget a link, given as it was made\n\n\
        A link is remembered. When it breaks, the kernel makes it again, for as long \
        as it takes; messages wait meanwhile. Two kernels with the same name cannot \
        be linked, neither directly nor through others.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis link node2\n\
        \x20 jarvis link -- ssh cannon jarvis link --stdio\n\
        \x20 jarvis link -- ssh -J login.cluster node0412 jarvis --on sweep link --stdio\n\
        \x20 jarvis link --remove node2")]
    Link(Link),

    /// Create a process on a kernel, or carry one on there
    #[command(long_about = "Create a process on a kernel, or carry one on there.\n\n\
        A process is NAME on a kernel: its address is `NAME@KERNEL`, and so is its \
        git branch, which is checked out as its own directory. A kernel runs a name \
        at most once. The same name may run on other kernels: those are other \
        instances of it, with branches of their own. Anyone can spawn: the human, or \
        a process during its turn. --on KERNEL says where; without it, here.\n\n\
        What happens depends on what is there:\n\
        \x20 NAME is stopped on that kernel      it is started again.\n\
        \x20 its branch `NAME@KERNEL` exists     it carries on from that branch.\n\
        \x20 neither                             a new process, starting with your choice of:\n\
        \x20   (no options)   Nothing. It does nothing, and costs nothing, until someone\n\
        \x20                  sends it a message; that message tells it what to be.\n\
        \x20   --prompt       Who it is and what it is for.\n\
        \x20   --from WHAT    The files and history of a branch: an expert's (`page`),\n\
        \x20                  a process's (`lammps@cannon-a`), or any tag or commit.\n\
        \x20   --tree DIR     A copy of a directory.\n\n\
        To move a process to another kernel, spawn it there from its branch here \
        (`jarvis spawn lammps --on node2 --from lammps@laptop`), then remove or stop \
        it here. Its files and notes come along; its conversation does not.\n\n\
        Unless it starts with nothing, it is woken at once by a first message from \
        you. Prints the address of the process.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis spawn scratch\n\
        \x20     A blank process, waiting for its first message.\n\n\
        \x20 jarvis spawn polaris --prompt \"You are our expert on the Polaris cluster.\"\n\
        \x20     An expert that starts from a prompt alone.\n\n\
        \x20 jarvis spawn page0 --from page\n\
        \x20     One more process of the expert `page`: the branch `page` is the code and\n\
        \x20     knowledge all of them share, and `page0@KERNEL` is where this one works.\n\n\
        \x20 jarvis spawn sweep-flux --on cannon-4471 --from flux --tag project=sweep\n\
        \x20     A process of the expert `flux`, on another kernel.\n\n\
        \x20 jarvis spawn gpu --from lammps1@laptop --prompt \"You work on the GPU kernels.\"\n\
        \x20     A fork of a process: what it has now, landed or not.\n\n\
        \x20 jarvis spawn sim --tree ~/code/sim --prompt-file brief.md\n\
        \x20     Bring in files that are not in the repository yet.")]
    Spawn(Spawn),

    /// Who exists and what each is doing; or everything about one process
    #[command(long_about = "Who exists and what each is doing; or everything about one process.\n\n\
        Without WHO: one line per process on every kernel known here, this kernel's \
        first, with its state, who made it, and how it describes itself (the first \
        line of its .jarvis/ABOUT.md). The state is:\n\
        \x20 idle           waiting for a message\n\
        \x20 thinking 3m    a turn is in progress, for this long\n\
        \x20 failing x4     its last 4 turns failed (an outage, a broken harness); its\n\
        \x20                mail is kept and its kernel keeps trying\n\
        \x20 stopped        someone ran `jarvis stop` on it; its mail waits\n\n\
        What is shown of another kernel's processes is what its records say, which \
        can be some seconds old, and stays as it was if that kernel was killed \
        (`jarvis kernels` shows when each last wrote).\n\n\
        With WHO: everything about that process: its branch and directory, who made \
        it, its service, what it has cost, how much mail is waiting, and how its last \
        turn went, with the whole error if it failed.")]
    #[command(after_long_help = "Examples:\n  jarvis ps\n  jarvis ps lammps\n  jarvis ps lammps@cannon-a\n  jarvis ps --json")]
    Ps(Ps),

    /// End what a process is doing now; its mail waits
    #[command(long_about = "End what a process is doing now; its mail waits.\n\n\
        The turn in progress, if any, is ended at once, together with the process's \
        service and everything either of them started. Nothing is deleted: its \
        directory, branch and mail stay as they are. Messages sent to it meanwhile \
        wait. `jarvis start` lets it go on; the messages of the turn that was cut \
        short are then given to it again, marked as such.")]
    #[command(after_long_help = "Examples:\n  jarvis stop lammps\n  jarvis stop lammps lammps-gpu polaris\n  jarvis stop sweep-flux@cannon-4471")]
    Stop(Who),

    /// Let a stopped process go on
    #[command(long_about = "Let a stopped process go on.\n\n\
        Its service is started again and its waiting mail is given to it. On a process \
        that is `failing`, stop followed by start makes the kernel try again at once \
        instead of after its pause.")]
    #[command(after_long_help = "Examples:\n  jarvis start lammps")]
    Start(Who),

    /// Take a process off its kernel: its directory and branch go, the records stay
    #[command(long_about = "Take a process off its kernel: its directory and branch go, the records stay.\n\n\
        What it is doing is ended first, as with `jarvis stop`. Then its directory \
        and its branch are deleted. Everything that was written down about it stays: \
        its messages and what it did can still be read. Mail that was waiting for it \
        is marked as dropped. Afterwards the name is free on that kernel.\n\n\
        Work it landed on an expert's branch is not touched. Work it had not landed \
        goes with its branch: the commit is printed, and `git branch NAME <commit>` \
        brings the files back for as long as git still has them (weeks, by default). \
        With --keep-branch the branch stays, and `jarvis spawn` with the same name \
        carries on from it. Use that when a process has moved to another kernel.\n\n\
        A process cannot remove itself.")]
    #[command(after_long_help = "Examples:\n  jarvis remove scratch\n  jarvis remove page0 page1\n  jarvis remove lammps-gpu --keep-branch")]
    Remove(Remove),

    /// Say what a process belongs to: a project, a place, anything
    #[command(long_about = "Say what a process belongs to: a project, a place, anything.\n\n\
        A tag is a name with a value, such as `project=moe` or `place=cluster one`. \
        A process has at most one value for each name. Tags are free-form: jarvis \
        stores and reports them (`jarvis ps`, `jarvis watch`) and gives them no \
        meaning. They are for people, processes and programs to find and group \
        processes by; the page arranges its map by them. Every process also counts \
        as tagged `kernel=` with the kernel it is on.\n\n\
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
        The message starts a turn at the recipient as soon as it is free. This command \
        does not wait for an answer: an answer, if one comes, is a new message to you \
        (see `jarvis messages`).\n\n\
        Sending is the only way to tell anyone anything. What an agent writes at the \
        end of its turn is logged and reaches nobody.\n\n\
        TO is a process. `lammps` means the one on this kernel and no other: if there \
        is none, the message is refused. A process on another kernel is always named \
        in full: `lammps@cannon-a`. To answer a message, send to the \
        address it came from. `human` is the person at this kernel; `human@laptop` \
        the person at that one.\n\n\
        Everything after TO is the text. With no text, or with `-`, the text is read \
        from standard input, which is the way to send several lines or anything with \
        quotes in it.\n\n\
        A message is never dropped. If the recipient cannot take it now (stopped, \
        failing), it waits there. If its kernel cannot be reached now, it waits on \
        the way and goes on when it can: `jarvis messages` shows how far it got. A \
        message to a kernel that said it closed, or to a name nobody has, is refused.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis send polaris How do I get an interactive GPU node?\n\n\
        \x20 jarvis send human@laptop - <<'EOF'\n\
        \x20 The build works on Polaris now.\n\
        \x20 Two tests still fail; details are in my notes.\n\
        \x20 EOF\n\n\
        \x20 git diff | jarvis send lammps@cannon-a")]
    Send(SendArgs),

    /// Read messages: all of them, or those to and from one process
    #[command(long_about = "Read messages: all of them, or those to and from one process.\n\n\
        Oldest first, ending with the newest. Each shows its name (the kernel it was \
        sent from, and its number there), the time, who sent it to whom, and how far \
        it has got if it is not dealt with yet:\n\
        \x20 on its way           not yet passed on from the kernel it was sent from\n\
        \x20 passed on            on its way through other kernels; not known to have arrived\n\
        \x20 waiting              arrived; the recipient has not taken it up\n\
        \x20 being worked on      a turn on it is in progress\n\
        \x20 could not be delivered, dropped\n\n\
        All messages every kernel ever put in the store can be read here, also of \
        kernels and processes that are gone. WHO as a bare name (`lammps`) means every \
        process of that name on any kernel, so that what was said to and by `lammps` \
        reads as one history; `lammps@cannon-a` means that one only. Use `human` for \
        what was said to and by the person.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis messages                       the last 20 messages between anyone\n\
        \x20 jarvis messages polaris -n 5          the last 5 to or from polaris\n\
        \x20 jarvis messages lammps --with polaris everything those two said to each other\n\
        \x20 jarvis messages --waiting             what has not been dealt with yet")]
    Messages(Messages),

    /// Look at the files of a process: its code and its notes
    #[command(long_about = "Look at the files of a process: its code and its notes.\n\n\
        Without PATH: the names of all files on the process's branch. With PATH: the \
        content of that file. This shows what is committed, which is everything up to \
        the end of the process's last turn. Reading is always allowed and disturbs \
        nobody; a process's notes are in its `.jarvis/` directory. It works for any \
        process whose branch is in this repository, also on kernels that are gone.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis show polaris\n\
        \x20 jarvis show polaris .jarvis/ABOUT.md\n\
        \x20 jarvis show lammps@cannon-a src/pair_lj.cpp")]
    Show(Show),

    /// What happened: every turn, message, commit and failure, in order
    #[command(long_about = "What happened: every turn, message, commit and failure, in order.\n\n\
        Kernels write down each thing they do or see as a record: a message sent, \
        passed on or arrived, a turn started, finished (with what it cost) or failed \
        (with the error), a commit, a service starting or exiting, a process being \
        made, stopped or started, a link coming up. This prints them in the order \
        this kernel learned of them, its own at once and other kernels' as they \
        reach the store. With WHO, only what that process did and the messages sent \
        to it.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis log                 the last 30 records\n\
        \x20 jarvis log -f              keep printing as things happen (Ctrl-C to leave)\n\
        \x20 jarvis log -fs page        watch what page's agent is doing, step by step\n\
        \x20 jarvis log lammps -n 100   the last 100 about lammps\n\
        \x20 jarvis log --json          for programs: every field of every record")]
    Log(Log),

    /// Follow every change as it happens, as JSON lines, for programs
    #[command(long_about = "Follow every change as it happens, as JSON lines, for programs.\n\n\
        Prints one JSON object per line and keeps running. It is how a program (a \
        page, a monitor, a process's own code) stays up to date without asking again \
        and again. The lines:\n\
        \x20 {\"kind\":\"process\",\"process\":{...}}   a process as `jarvis ps --json` shows it:\n\
        \x20                                         every process at the start, then each one\n\
        \x20                                         whenever something about it changes\n\
        \x20 {\"kind\":\"gone\",\"id\":ADDRESS}          a process was removed, or its kernel closed\n\
        \x20 {\"kind\":\"ready\",\"cursor\":N}           the start is complete; facts after N follow\n\
        \x20 {\"kind\":\"fact\",\"fact\":{...}}         a record as `jarvis log --json` shows it. One\n\
        \x20                                         that brings a message (`send`, `arrive`)\n\
        \x20                                         also has `message`: the message\n\
        \x20 {\"kind\":\"message\",\"message\":{...}}   a message whose status changed\n\n\
        Facts come in order, each exactly once. Without --after, only facts from now \
        on; with it, also the earlier ones above that number. A program that is cut \
        off starts again with --after set to the `id` of the last fact it saw.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis watch                  everything from now on\n\
        \x20 jarvis watch --steps          also each step agents take\n\
        \x20 jarvis watch --after 5120     continue after fact 5120")]
    Watch(Watch),

    /// Look after the store, where the records are kept
    #[command(long_about = "Look after the store, where the records are kept.\n\n\
        Every kernel writes what happens to it into the store, a directory, as small \
        files that are never changed afterwards: `STORE/<kernel>/log/<first>-<last>.jsonl`, \
        one record per line. A record is named by its kernel and number \
        (`laptop:412`), and two copies of a record are always identical. So stores \
        can be copied, put together and read in any order, and nothing in them needs \
        a lock. A kernel reads the whole store, and that is how it knows what \
        happened on other kernels, running or gone.\n\n\
        These commands work on the store's files directly. None needs a kernel, and \
        the kernels using the store need not be told: they read whatever is there.\n\n\
        \x20 jarvis store ship          have this kernel write what it has not written yet,\n\
        \x20                            now (it does so by itself every few seconds)\n\
        \x20 jarvis store compact       put everything into one read-only SQLite file,\n\
        \x20                            `STORE/packs/*.db`, and delete the files it replaces\n\
        \x20 jarvis store merge OTHER   copy in what another store has and this one lacks\n\n\
        The store is yours to look after: files may be deleted, moved to an archive, \
        or read with any tool. What is deleted here is gone from this store only; a \
        copy elsewhere that still has it brings it back when merged in. Tell the \
        others before you do something they could trip over.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis store compact\n\
        \x20 jarvis store compact --without laptop:412     leave a record out for good\n\
        \x20 jarvis store merge /mnt/backup/jarvis-store\n\
        \x20 jarvis messages --store /mnt/backup/jarvis-store lammps    read a store with no kernel")]
    Store(StoreArgs),
}

#[derive(Args)]
pub struct Up {
    /// The kernel's name. You choose it; it must never have been used by another kernel
    pub name: String,

    /// The git repository that holds the code [default: the one this directory is in]
    #[arg(long, value_name = "DIR")]
    pub repo: Option<String>,

    /// Where this kernel keeps its own database and its processes' working
    /// directories [default: .jarvis/kernel/NAME in the repository]
    #[arg(long, value_name = "DIR")]
    pub state: Option<String>,

    /// Write new records to the store this often, in seconds. Lower loses less
    /// if the machine dies; higher makes fewer small files
    #[arg(long, value_name = "SECONDS", default_value_t = 2.0)]
    pub ship_every: f64,

    /// Look in the store for other kernels' records this often, in seconds
    #[arg(long, value_name = "SECONDS", default_value_t = 2.0)]
    pub scan_every: f64,
}

#[derive(Args)]
pub struct Kernels {
    /// Write down that you take this kernel to be gone
    #[arg(long, value_name = "NAME")]
    pub close: Option<String>,

    /// Print JSON instead of a table
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct Link {
    /// A kernel on this machine to link to
    pub kernel: Option<String>,

    /// Be the far end of a link made through a command: pass what arrives on
    /// standard input to the kernel here, and its answers to standard output
    #[arg(long, conflicts_with_all = ["kernel", "remove"])]
    pub stdio: bool,

    /// Forget the link instead of making it
    #[arg(long)]
    pub remove: bool,

    /// Print JSON instead of a table
    #[arg(long)]
    pub json: bool,

    /// A command through which to reach another kernel
    #[arg(last = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

#[derive(Args, Clone)]
pub struct Spawn {
    /// What to call it. Letters, digits, `-`, `_` and `.` only
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
              land their finished work on. A process's own branch is its address, \
              `NAME@KERNEL`. WHAT is tried as something git knows first, then as the name of \
              a process on this kernel, then as a branch of \
              that name on the remote (`origin/WHAT`).")]
    pub from: Option<String>,

    /// Start from a copy of a directory of files
    #[arg(long, value_name = "DIR",
          long_help = "Start from a copy of a directory of files.\n\n\
              Everything in DIR is copied, hidden files included, and becomes the first \
              commit of the new process. Files a `.gitignore` in DIR excludes are copied but \
              not committed. DIR must not be the top of a git repository; for code that git \
              already has, use --from. DIR is looked for on the machine of the kernel that \
              creates the process.")]
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
    /// The process
    pub who: String,

    /// Tags to set, as NAME=VALUE; NAME= removes one
    #[arg(value_name = "NAME=VALUE")]
    pub tags: Vec<String>,
}

#[derive(Args)]
pub struct Ps {
    /// Show everything about this one process
    pub who: Option<String>,

    /// Also list the processes of kernels that closed
    #[arg(long)]
    pub all: bool,

    /// Print JSON instead of a table
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct Who {
    /// One or more processes
    #[arg(required = true)]
    pub who: Vec<String>,
}

#[derive(Args)]
pub struct SendArgs {
    /// Who it is for: a process, or `human`
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
    /// One or more processes
    #[arg(required = true, value_name = "WHO")]
    pub who: Vec<String>,

    /// Keep its git branch, so that its files and history stay
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

    /// Only messages sent after this one (a message's name, such as laptop:412)
    #[arg(long, value_name = "MESSAGE")]
    pub after: Option<String>,

    /// Only messages sent before this one (to read further back)
    #[arg(long, value_name = "MESSAGE")]
    pub before: Option<String>,

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
    /// The process
    pub who: String,

    /// A file in its directory. Without it, the names of all its files
    pub path: Option<String>,
}

#[derive(Args)]
pub struct Log {
    /// Only what this process did, and the messages sent to it
    pub who: Option<String>,

    /// Only records with a number above this
    #[arg(long, value_name = "NUMBER")]
    pub after: Option<i64>,

    /// Only records with a number below this
    #[arg(long, value_name = "NUMBER")]
    pub before: Option<i64>,

    /// How many, counted from the newest [default: 30, or 500 with --json]
    #[arg(short = 'n', value_name = "COUNT")]
    pub count: Option<i64>,

    /// Keep running and print new records as they happen
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

#[derive(Args)]
pub struct StoreArgs {
    #[command(subcommand)]
    pub what: StoreCmd,
}

#[derive(Subcommand)]
pub enum StoreCmd {
    /// Have this kernel write its new records to the store now
    Ship,

    /// Put everything in the store into one read-only SQLite file
    #[command(long_about = "Put everything in the store into one read-only SQLite file.\n\n\
        Reads every log file and every earlier pack, writes one new pack beside them, \
        puts it in place under its final name in one step, and only then deletes the \
        files it replaces. If it is interrupted, records are at worst in two places, \
        which harms nothing. Kernels may keep running: what they write meanwhile is \
        left alone, and they read the pack like any other file.\n\n\
        The pack has two tables: `records (k, n, at, kind, proc, data)` and \
        `kernels (name, identity, created_at)`. Any SQLite tool can read it.")]
    Compact {
        /// Leave this record out (as KERNEL:NUMBER). It is then gone from this store. Can be given several times
        #[arg(long, value_name = "RECORD")]
        without: Vec<String>,
    },

    /// Copy in what another store has and this one lacks
    Merge {
        /// The other store: a directory
        other: String,
    },
}
