//! The command line: every command, its arguments, and its help.
//!
//! The help is written here, next to the arguments it describes, and is the
//! one description of how to use jarvis. People read it with `jarvis help`;
//! agents are shown the same text. Each command is a struct whose fields are
//! its arguments (this is clap's "derive" style): the `///` comment on a
//! field is that argument's help.

use clap::{Args, CommandFactory, Parser, Subcommand};

/// The version and the commit it was built from, such as `0.1.0 (c10ce80)`.
pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("JARVIS_BUILD"), ")");

#[derive(Parser)]
#[command(name = "jarvis", version = VERSION, about = "Many long-lived processes, each with its own code and agent, working together for one human.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

/// The commands in the order they are listed, under these headings.
const GROUPS: &[(&str, &[&str])] = &[
    ("Kernels", &["up", "kernels", "link"]),
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
        "jarvis: one system of many long-lived processes. Each process has a name, its\n\
         own directory of code and notes (a git branch), and an agent that can change\n\
         them. Processes are hosted by kernels, which run on many machines and are\n\
         connected by links. Processes work together, by sending each other messages,\n\
         to get the best results for one human.\n",
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
        "\nAddresses. A process is always named in full: `lammps@cannon-a` is the process\n\
         `lammps` on the kernel `cannon-a`. `human@laptop` is the person at the kernel\n\
         `laptop`. `@cannon-a` is the kernel itself, with everything on it. An address\n\
         means the same wherever it is used, and jarvis finds the way there.\n\
         \n\
         Where a command looks. `ps`, `messages`, `log` and `watch` look at the kernel\n\
         you are on, unless you name processes or kernels, or say --all for every kernel\n\
         known here. Each kernel answers for itself. One that does not answer or cannot\n\
         be reached is named on standard error, and the command then exits with 2: what\n\
         was shown is then not everything.\n\
         \n\
         Which kernel you are on. A process is on the kernel that hosts it. Otherwise it\n\
         is the only kernel running on this machine; if there are several, set\n\
         $JARVIS_KERNEL to the name of one.\n\
         \n\
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
        \x20 - passes on what is for other kernels, over its links (see `jarvis help link`);\n\
        \x20 - writes down everything that happens to it, and answers questions about it.\n\n\
        NAME is the kernel's name, and you choose it. It is the second half of the \
        address of every process it hosts (`lammps@NAME`) and of their branches, so \
        it must be different from every kernel this one will ever be linked to or \
        share a repository with, also ones that are gone. Starting a kernel again \
        under its name carries on from its database in the store, on any machine \
        that has the store. A name whose database is gone from the store cannot be \
        taken again: choose a new one.\n\n\
        What a kernel uses:\n\
        \x20 the repository   all code, as git branches: the one you are in, or --repo.\n\
        \x20                  Many kernels can share one.\n\
        \x20 the store        what lasts: $JARVIS_STORE, or `.jarvis` in the repository.\n\
        \x20                  Each kernel has its database there (`kernel/NAME/kernel.db`:\n\
        \x20                  the only one there is, and what a kernel starts from) and\n\
        \x20                  its processes' directories (`kernel/NAME/proc/`).\n\
        \x20 scratch          $JARVIS_SCRATCH, if set: a directory on the machine's own\n\
        \x20                  disk, which may vanish. The kernel then works on a copy of\n\
        \x20                  its database there and saves it to the store when something\n\
        \x20                  changed: every $JARVIS_CHECKPOINT_SECONDS (60 if not set),\n\
        \x20                  and when it stops. Set it where the store is on network\n\
        \x20                  storage. Not set, the kernel works in the store directly.\n\
        \x20 its socket       where commands reach it: `NAME.sock` in $JARVIS_RUN, or in\n\
        \x20                  `/tmp/jarvis-<uid>`. `jarvis` must look in the same place.\n\n\
        Ctrl-C, or the signal TERM to its process (`jarvis kernels` shows which), \
        stops it, together with every service and every turn in progress. Nothing is lost: messages that were being worked on are \
        given again, marked as such, the next time it runs. The same holds if it is \
        killed, except with scratch: then it starts from what was last saved, and \
        what it did after that is lost. Nothing starts a kernel again by itself.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis up laptop\n\
        \x20 JARVIS_STORE=/n/proj/jarvis-store JARVIS_SCRATCH=/tmp/$USER/jarvis jarvis up cannon-sweep --repo /n/proj/jarvis\n\
        \x20 jarvis up laptop 2> kernel.log     # what happens is printed on standard error")]
    Up(Up),

    /// The kernels this one has heard of, and whether each can be reached
    #[command(long_about = "The kernels this one has heard of, and whether each can be reached.\n\n\
        One line per kernel: this one, those that can be reached over links now (and \
        through which neighbour, how many links away), and those that could be reached \
        before. For each, the version of jarvis it runs.\n\n\
        A kernel that was killed never says so: it just cannot be reached. Mail for it \
        waits, for as long as it takes. Whether it is gone for good is for you to \
        judge. --close NAME says that you take it to be gone: what was waiting here \
        for it is given up (whoever sent it is told), and new mail for it is refused. \
        If it is reached again, it counts as there again.")]
    #[command(after_long_help = "Examples:\n  jarvis kernels\n  jarvis kernels --close cannon-4471")]
    Kernels(Kernels),

    /// Connect this kernel to another
    #[command(long_about = "Connect this kernel to another.\n\n\
        A link carries messages, requests and questions both ways. Kernels pass on \
        what is not for them, so two kernels need not be linked directly: a path of \
        links is enough. Making a link takes one side only.\n\n\
        \x20 jarvis link                    list this kernel's links, and why any is down\n\
        \x20 jarvis link NAME               link to the kernel NAME on this machine. For a\n\
        \x20                                link that is there and down: try it again now\n\
        \x20 jarvis link -- COMMAND ...     link through a command: this kernel runs it, and\n\
        \x20                                talks to the other kernel through its input and\n\
        \x20                                output. The command must end up running\n\
        \x20                                `jarvis link --stdio` where the other kernel is.\n\
        \x20 jarvis link --remove ...       forget a link, given as it was made\n\
        \x20 jarvis link --stdio [NAME]     be the far end of such a command: connect\n\
        \x20                                standard input and output to the kernel NAME\n\
        \x20                                on this machine. It fails if NAME is not running.\n\n\
        A link is remembered. When it breaks, the kernel makes it again: after 2 \
        seconds, then 4, doubling up to once a minute, for as long as it takes. What \
        is on its way waits meanwhile. `jarvis link` shows, for a link that is down, \
        what the command last printed as its error, how often it was tried, and when \
        it is tried next. A link that says nothing for 70 seconds is taken down and \
        made again.\n\n\
        Two kernels with the same name cannot be linked, neither directly nor through \
        others.\n\n\
        Over ssh, the command runs with ssh's own short search path, so `jarvis` is \
        often not found there: name it in full (`.cargo/bin/jarvis`), or run a script \
        on the other machine that sets the path. `-o BatchMode=yes` makes ssh fail \
        instead of asking for a password nobody is there to type. The far end must \
        look for the kernel's socket where that kernel put it (`jarvis help up`).")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis link node2\n\
        \x20 jarvis link -- ssh -o BatchMode=yes cannon .cargo/bin/jarvis link --stdio\n\
        \x20 jarvis link -- ssh -o BatchMode=yes cannon bin/link-into-job sweep\n\
        \x20     A script of your own on the other machine, here one that finds the job the\n\
        \x20     kernel `sweep` runs in and starts `jarvis link --stdio sweep` inside it. A\n\
        \x20     link is made again with the same command each time, so the command must\n\
        \x20     not name anything that changes, such as a job's number.\n\
        \x20 jarvis link --remove node2")]
    Link(Link),

    /// Create a process on a kernel, or carry one on there
    #[command(long_about = "Create a process on a kernel, or carry one on there.\n\n\
        ADDRESS is `NAME@KERNEL`: the process NAME on the kernel KERNEL. It is also \
        the name of its git branch, which is checked out as its own directory. A \
        kernel runs a name at most once. The same name may run on other kernels: \
        those are other instances of it, with branches of their own. Anyone can \
        spawn, on any kernel: the human, or a process during its turn.\n\n\
        What happens depends on what is there:\n\
        \x20 NAME is stopped on that kernel      it is started again.\n\
        \x20 its branch `NAME@KERNEL` exists     it carries on from that branch.\n\
        \x20 neither                             a new process, starting with your choice of:\n\
        \x20   (no options)   Nothing. It does nothing, and costs nothing, until someone\n\
        \x20                  sends it a message; that message tells it what to be.\n\
        \x20   --prompt       Who it is and what it is for.\n\
        \x20   --from WHAT    The files and history of a branch: an expert's (`page`),\n\
        \x20                  a process's (`lammps@cannon-a`), or any tag or commit.\n\n\
        Code is in git and nowhere else: a process starts from a commit or from \
        nothing. WHAT is taken from the repository of the kernel that creates the \
        process if it is a branch, tag or commit there. Otherwise it is fetched, at \
        that moment, from the remote `origin`, so the process starts from what was \
        last pushed; if it is not there either, push it first. Nothing is fetched at \
        any other time, and nothing is pushed for you.\n\n\
        To move a process to another kernel, spawn it there from its branch here \
        (`jarvis spawn lammps@node2 --from lammps@laptop`), then remove or stop it \
        here. Its files and notes come along; its conversation does not.\n\n\
        Unless it starts with nothing, its agent is woken at once by a notice from \
        its kernel. Prints the address of the process.\n\n\
        A process on another kernel is created by that kernel: the request is carried \
        there like a message, and waits on the way if a link is down. If the answer \
        is not back within 20 seconds, the command says so and exits with 2; the \
        request is still carried out when it gets there, and what came of it is in \
        `jarvis messages`.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis spawn scratch@laptop\n\
        \x20     A blank process, waiting for its first message.\n\n\
        \x20 jarvis spawn polaris@laptop --prompt \"You are our expert on the Polaris cluster.\"\n\
        \x20     An expert that starts from a prompt alone.\n\n\
        \x20 jarvis spawn page0@laptop --from page\n\
        \x20     One more process of the expert `page`: the branch `page` is the code and\n\
        \x20     knowledge all of them share, and `page0@laptop` is where this one works.\n\n\
        \x20 jarvis spawn flux@cannon-sweep --from flux --tag project=sweep\n\
        \x20     A process of the expert `flux`, on another kernel.\n\n\
        \x20 jarvis spawn gpu@laptop --from lammps1@laptop --prompt-file brief.md\n\
        \x20     A fork of a process: what it has now, landed or not.\n\n\
        \x20 jarvis spawn counter@node2 --from counter --harness none\n\
        \x20     A process with no agent: its code answers its messages.")]
    Spawn(Spawn),

    /// Who exists and what each is doing; or everything about one process
    #[command(long_about = "Who exists and what each is doing; or everything about one process.\n\n\
        \x20 jarvis ps                    the processes on this kernel\n\
        \x20 jarvis ps @cannon-a @node2   those on these kernels\n\
        \x20 jarvis ps --all              those on every kernel known here\n\
        \x20 jarvis ps a@node2 b@node3    these processes only\n\
        \x20 jarvis ps lammps@cannon-a    one process: everything about it\n\n\
        The list has one line per process, with its state, who made it, and how it \
        describes itself (the first line of its .jarvis/ABOUT.md). The state is:\n\
        \x20 idle           waiting for a message\n\
        \x20 thinking 3m    a turn is in progress, for this long\n\
        \x20 failing x4     its last 4 turns failed (an outage, a broken harness); its\n\
        \x20                mail is kept and its kernel keeps trying\n\
        \x20 stopped        someone ran `jarvis stop` on it; its mail waits\n\n\
        Everything about one process is: its branch and directory, who made it, its \
        service, what it has cost, how much mail is waiting, and how its last turn \
        went, with the whole error if it failed.\n\n\
        Every kernel answers for its own processes, now.\n\n\
        --json prints a list of objects, one per process: `id` (the address), `name`, \
        `kernel`, `state` (idle, thinking, failing, stopped), `about`, `parent`, \
        `from`, `tags`, `branch`, `harness`, `created_at`, `turns`, `cost_usd`, \
        `mail_waiting`, `busy_since`, `failures`, `directory`, `has_run`, \
        `has_handle`. For one process it prints that one object, with `commit`, \
        `service_pid` and `last_turn` as well. Times are milliseconds since 1970.")]
    #[command(after_long_help = "Examples:\n  jarvis ps\n  jarvis ps --all\n  jarvis ps lammps@cannon-a\n  jarvis ps @cannon-a --json")]
    Ps(Ps),

    /// End what a process is doing now; its mail waits
    #[command(long_about = "End what a process is doing now; its mail waits.\n\n\
        The turn in progress, if any, is ended at once, together with the process's \
        service and everything either of them started. Nothing is deleted: its \
        directory, branch and mail stay as they are. Messages sent to it meanwhile \
        wait. `jarvis start` lets it go on; the messages of the turn that was cut \
        short are then given to it again, marked as such.\n\n\
        A process on another kernel is stopped by that kernel; see `jarvis help spawn` \
        for how such a request travels.")]
    #[command(after_long_help = "Examples:\n  jarvis stop lammps@laptop\n  jarvis stop lammps@laptop gpu@laptop flux@cannon-sweep")]
    Stop(Who),

    /// Let a stopped process go on
    #[command(long_about = "Let a stopped process go on.\n\n\
        Its service is started again and its waiting mail is given to it. On a process \
        that is `failing`, stop followed by start makes the kernel try again at once \
        instead of after its pause.")]
    #[command(after_long_help = "Examples:\n  jarvis start lammps@laptop")]
    Start(Who),

    /// Take a process off its kernel: its directory and branch go, what was written down stays
    #[command(long_about = "Take a process off its kernel: its directory and branch go, what was written down stays.\n\n\
        What it is doing is ended first, as with `jarvis stop`. Then its directory \
        and its branch are deleted. Everything that was written down about it stays: \
        its messages and what it did can still be read. Mail that was waiting for it \
        is marked as dropped. Afterwards the name is free on that kernel.\n\n\
        Work it landed on an expert's branch is not touched. Work it had not landed \
        goes with its branch: the commit is printed, and `git branch NAME <commit>` \
        brings the files back for as long as git still has them (weeks, by default). \
        With --keep-branch the branch stays, and `jarvis spawn` with the same address \
        carries on from it. Use that when a process has moved to another kernel.\n\n\
        A process cannot remove itself.")]
    #[command(after_long_help = "Examples:\n  jarvis remove scratch@laptop\n  jarvis remove page0@laptop page1@laptop\n  jarvis remove lammps@laptop --keep-branch")]
    Remove(Remove),

    /// Say what a process belongs to: a project, a place, anything
    #[command(long_about = "Say what a process belongs to: a project, a place, anything.\n\n\
        A tag is a name with a value, such as `project=moe` or `place=cluster one`. \
        A process has at most one value for each name. Tags are free-form: jarvis \
        stores and reports them (`jarvis ps`, `jarvis watch`) and gives them no \
        meaning. They are for people, processes and programs to find and group \
        processes by. Every process also counts as tagged `kernel=` with the kernel \
        it is on.\n\n\
        Anyone may tag any process. `NAME=` with nothing after it removes that tag. \
        Without any tags given, the process's tags are printed.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis tag lammps1@laptop project=moe\n\
        \x20 jarvis tag lammps1@laptop \"place=cluster one\" role=lead\n\
        \x20 jarvis tag lammps1@laptop role=            remove the tag `role`\n\
        \x20 jarvis tag lammps1@laptop                  print its tags")]
    Tag(Tag),

    /// Send a message to a process, or to the human
    #[command(long_about = "Send a message to a process, or to the human.\n\n\
        The message starts a turn at the recipient as soon as it is free. This command \
        does not wait for an answer: an answer, if one comes, is a new message to you \
        (see `jarvis messages`).\n\n\
        Sending is the only way to tell anyone anything. What an agent writes at the \
        end of its turn is logged and reaches nobody.\n\n\
        TO is an address: `lammps@cannon-a`, or `human@laptop` for the person at the \
        kernel `laptop`. To answer a message, send to the address it came from.\n\n\
        Everything after TO is the text. With no text, or with `-`, the text is read \
        from standard input, which is the way to send several lines or anything with \
        quotes in it.\n\n\
        A message is never dropped, and it arrives once. If the recipient cannot take \
        it now (stopped, failing), it waits there. If its kernel cannot be reached \
        now, it waits on the way and goes on when it can. `jarvis messages` shows how \
        far it got, also on other kernels: the kernel it went to says when it arrived \
        and when it was dealt with. A message that can never arrive (nobody of that \
        name is there) comes back as a notice to you. A message to a kernel that \
        nobody here has heard of, or that was closed, is refused at once.\n\n\
        Prints `sent message ID to TO`; ID names the message, such as `laptop:412`. \
        Options (--as, --json) go before TO: everything after TO is the text.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis send polaris@laptop How do I get an interactive GPU node?\n\n\
        \x20 jarvis send human@laptop - <<'EOF'\n\
        \x20 The build works on Polaris now.\n\
        \x20 Two tests still fail; details are in my notes.\n\
        \x20 EOF\n\n\
        \x20 git diff | jarvis send lammps@cannon-a\n\n\
        \x20 jarvis send --json polaris@laptop Are you there?\n\
        \x20     {\"id\":\"laptop:412\",\"to\":\"polaris@laptop\",\"note\":null}")]
    Send(SendArgs),

    /// Read messages: those of a kernel, or those to and from one process
    #[command(long_about = "Read messages: those of a kernel, or those to and from one process.\n\n\
        \x20 jarvis messages                    every message this kernel sent or received\n\
        \x20 jarvis messages lammps@cannon-a    those to and from that process (asked of\n\
        \x20                                    its kernel)\n\
        \x20 jarvis messages @cannon-a          every message that kernel sent or received\n\
        \x20 jarvis messages --all              those of every kernel known here\n\n\
        Oldest first, ending with the newest. Each shows its name (the kernel it was \
        sent from, and its number there), the time, who sent it to whom, and how far \
        it has got if it is not dealt with yet:\n\
        \x20 on its way           not yet passed on from the kernel it was sent from\n\
        \x20 passed on            on its way through other kernels\n\
        \x20 waiting              arrived; the recipient has not taken it up\n\
        \x20 being worked on      a turn on it is in progress\n\
        \x20 could not be delivered, dropped, could not be carried out\n\n\
        A request to another kernel (`jarvis stop lammps@cannon-a` typed elsewhere) \
        is shown as a message to that kernel, `kernel@cannon-a`, followed by what the \
        kernel answered.\n\n\
        --json prints a list of objects: `id`, `at`, `from`, `to`, `body`, `status` \
        (sent, forwarded, pending, taken, handled, dropped, undeliverable, failed), \
        `attempts`, `result`, `kernel` (who answered) and `seq` (the number of the \
        fact that brought it there, as in `jarvis log --json`).")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis messages                                    the last 20 messages here\n\
        \x20 jarvis messages polaris@laptop -n 5                the last 5 to or from polaris\n\
        \x20 jarvis messages lammps@laptop --with polaris@laptop    what those two said to each other\n\
        \x20 jarvis messages --waiting --all                    what has not been dealt with, anywhere")]
    Messages(Messages),

    /// Look at the files of a process: its code and its notes
    #[command(long_about = "Look at the files of a process: its code and its notes.\n\n\
        Without PATH: the names of all files on the process's branch. With PATH: the \
        content of that file. This shows what is committed, which is everything up to \
        the end of the process's last turn. Reading is always allowed and disturbs \
        nobody; a process's notes are in its `.jarvis/` directory.\n\n\
        The branch is read from this kernel's repository if it is there (kernels of \
        one site share a repository, and a branch outlives its process and its \
        kernel); otherwise the process's own kernel is asked.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis show polaris@laptop\n\
        \x20 jarvis show polaris@laptop .jarvis/ABOUT.md\n\
        \x20 jarvis show lammps@cannon-a src/pair_lj.cpp")]
    Show(Show),

    /// What happened: every turn, message, commit and failure, in order
    #[command(long_about = "What happened: every turn, message, commit and failure, in order.\n\n\
        A kernel writes down each thing it does or sees as a record: a message sent, \
        passed on or arrived, a turn started, finished (with what it cost) or failed \
        (with the error), a commit, a service starting or exiting, a process being \
        made, stopped or started, a link coming up or failing.\n\n\
        \x20 jarvis log                     what happened on this kernel\n\
        \x20 jarvis log lammps@cannon-a     what that process did, and the messages sent\n\
        \x20                                to it (asked of its kernel)\n\
        \x20 jarvis log @cannon-a           what happened on that kernel\n\
        \x20 jarvis log --all               on every kernel known here, by time\n\n\
        With -f, what was written while a kernel could not be reached is printed when \
        it can be again, and so after newer lines of other kernels: the time on each \
        line is when it happened.\n\n\
        --json prints a list of objects: `id` (the record's place in its kernel's \
        order), `kernel`, `n` (its number there; empty for a step), `at`, `proc` (who \
        it is about), `kind` and `data`.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis log                       the last 30 records here\n\
        \x20 jarvis log -f --all              keep printing as things happen anywhere (Ctrl-C to leave)\n\
        \x20 jarvis log -fs page0@laptop      watch what page0's agent is doing, step by step\n\
        \x20 jarvis log lammps@cannon-a -n 100")]
    Log(Log),

    /// Follow every change as it happens, as JSON lines, for programs
    #[command(long_about = "Follow every change as it happens, as JSON lines, for programs.\n\n\
        Prints one JSON object per line and keeps running. It is how a program (a \
        page, a monitor, a process's own code) stays up to date without asking again \
        and again. What is followed is said as for `jarvis ps`: this kernel, or the \
        processes and kernels named, or --all. Kernels that become reachable while \
        --all is running are followed from then on.\n\n\
        Every line has `kernel`: the kernel it is from. The lines:\n\
        \x20 {\"kind\":\"process\",\"process\":{...}}   a process as `jarvis ps --json` shows it:\n\
        \x20                                         every process at the start, then each one\n\
        \x20                                         whenever something about it changes\n\
        \x20 {\"kind\":\"gone\",\"id\":ADDRESS}          a process was removed\n\
        \x20 {\"kind\":\"ready\",\"cursor\":N}           that kernel's start is complete; its facts\n\
        \x20                                         after N follow\n\
        \x20 {\"kind\":\"fact\",\"fact\":{...}}         a record as `jarvis log --json` shows it. One\n\
        \x20                                         that brings a message (`send`, `arrive`)\n\
        \x20                                         also has `message`: the message\n\
        \x20 {\"kind\":\"message\",\"message\":{...}}   a message whose status changed\n\
        \x20 {\"kind\":\"lost\",\"why\":TEXT}            that kernel cannot be reached any more.\n\
        \x20                                         When it can again, it is followed again,\n\
        \x20                                         from where it stopped\n\n\
        A kernel's facts come in order, each exactly once. A message between two \
        kernels is reported by both. Without --after, only facts from now on; with \
        `--after KERNEL:N`, also that kernel's earlier facts above N. A program that \
        is cut off starts again with --after set, for each kernel, to the `id` of the \
        last fact it saw from it.")]
    #[command(after_long_help = "Examples:\n\
        \x20 jarvis watch                              this kernel, from now on\n\
        \x20 jarvis watch --all --steps                everything reachable, also each step agents take\n\
        \x20 jarvis watch a@node2 b@node3              two processes\n\
        \x20 jarvis watch --all --after laptop:5120    continue laptop's facts after 5120")]
    Watch(Watch),
}

#[derive(Args)]
pub struct Up {
    /// The kernel's name. You choose it; it must never have been used by another kernel
    pub name: String,

    /// The git repository that holds the code [default: the one this directory is in]
    #[arg(long, value_name = "DIR")]
    pub repo: Option<String>,
}

#[derive(Args)]
pub struct Kernels {
    /// Say that you take this kernel to be gone
    #[arg(long, value_name = "NAME")]
    pub close: Option<String>,

    /// Print JSON instead of a table
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct Link {
    /// A kernel on this machine: the one to link to, or with --stdio the one to be the far end for
    pub kernel: Option<String>,

    /// Be the far end of a link made through a command: pass what arrives on
    /// standard input to the kernel here, and its answers to standard output
    #[arg(long, conflicts_with = "remove")]
    pub stdio: bool,

    /// Forget the link instead of making it
    #[arg(long)]
    pub remove: bool,

    /// Print JSON instead of a list
    #[arg(long)]
    pub json: bool,

    /// A command through which to reach another kernel
    #[arg(last = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

#[derive(Args, Clone)]
pub struct Spawn {
    /// The process: NAME@KERNEL. NAME is letters, digits, `-`, `_` and `.` only
    #[arg(value_name = "ADDRESS")]
    pub address: String,

    /// Who it is and what it is for. Saved as .jarvis/PROMPT.md in its directory
    #[arg(long, value_name = "TEXT", conflicts_with = "prompt_file")]
    pub prompt: Option<String>,

    /// The same, read from a file
    #[arg(long, value_name = "FILE")]
    pub prompt_file: Option<String>,

    /// Start from a git branch (an expert's, or a process's), tag or commit
    #[arg(long, value_name = "WHAT",
          long_help = "Start from a git branch (an expert's, or a process's), tag or commit.\n\n\
              The new process gets the same files and the same history on a branch of its own, \
              so work can later be merged in either direction. Everything comes along, the \
              `.jarvis/` directory included (prompt, notes, service, tools), but no \
              conversation: it starts with a fresh memory.\n\n\
              An expert is a branch with a plain name, such as `page`, that its processes \
              land their finished work on. A process's own branch is its address, \
              `NAME@KERNEL`. WHAT is a branch, tag or commit in the repository of the kernel that \
              creates the process; or else a branch on its remote `origin`, which is fetched \
              at that moment.")]
    pub from: Option<String>,

    /// Which agent harness runs it: the name of a file in `harness/`, or `none`
    #[arg(long, value_name = "NAME", default_value = "claude",
          long_help = "Which agent harness runs it: the name of a file in `harness/`, or `none`.\n\n\
              With `none` the process has no agent. Its code deals with its messages \
              (`.jarvis/handle`), and it is never sent notices meant for an agent. A message \
              its code does not take goes back to whoever sent it, as not read.")]
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
    /// Processes (NAME@KERNEL) and kernels (@KERNEL) to look at [default: this kernel]
    #[arg(value_name = "WHAT")]
    pub what: Vec<String>,

    /// Every kernel known here; one that cannot be reached is named
    #[arg(long, conflicts_with = "what")]
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
    /// Who it is for: a process, or the human, as an address
    pub to: String,

    /// The message. With none, or `-`, it is read from standard input
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub text: Vec<String>,

    /// Send as someone else, such as `human@laptop`. For programs that pass on what a person typed
    #[arg(long = "as", value_name = "WHO")]
    pub sender: Option<String>,

    /// Print JSON instead of a sentence: `{"id": the message's name, "to": TO,
    /// "note": why it will not be read soon, if so}`
    #[arg(long)]
    pub json: bool,
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
    /// A process (or `human@KERNEL`): messages to or from it. Or @KERNEL: that kernel's messages [default: this kernel's]
    #[arg(value_name = "WHAT")]
    pub what: Option<String>,

    /// Every kernel known here; one that cannot be reached is named
    #[arg(long, conflicts_with = "what")]
    pub all: bool,

    /// Only messages that have not been dealt with yet
    #[arg(long)]
    pub waiting: bool,

    /// Only messages between the process and this other one
    #[arg(long, value_name = "OTHER", requires = "what")]
    pub with: Option<String>,

    /// Only messages sent after this moment: a time in milliseconds since
    /// 1970 (`at` in --json), or the name of a message this kernel has
    #[arg(long, value_name = "WHEN")]
    pub after: Option<String>,

    /// Only messages sent before this moment (to read further back)
    #[arg(long, value_name = "WHEN")]
    pub before: Option<String>,

    /// Instead of messages: for each two who have written to each other, how
    /// many messages, how many of them waiting, how many written by jarvis,
    /// and when the last one was
    #[arg(long, conflicts_with_all = ["waiting", "with"])]
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
    /// Processes (NAME@KERNEL) and kernels (@KERNEL) to follow [default: this kernel]
    #[arg(value_name = "WHAT")]
    pub what: Vec<String>,

    /// Every kernel known here, and those that come into reach later
    #[arg(long, conflicts_with = "what")]
    pub all: bool,

    /// Also print a kernel's earlier facts above this number: KERNEL:NUMBER. Can be given once per kernel
    #[arg(long, value_name = "KERNEL:NUMBER")]
    pub after: Vec<String>,

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
    /// A process: what it did, and the messages sent to it. Or @KERNEL: what happened there [default: this kernel]
    #[arg(value_name = "WHAT")]
    pub what: Option<String>,

    /// Every kernel known here; one that cannot be reached is named
    #[arg(long, conflicts_with = "what")]
    pub all: bool,

    /// Only records after this place in the kernel's order (`id` in --json).
    /// For one kernel or one process only
    #[arg(long, value_name = "NUMBER", conflicts_with = "all")]
    pub after: Option<i64>,

    /// Only records before this place in the kernel's order
    #[arg(long, value_name = "NUMBER", conflicts_with = "all")]
    pub before: Option<i64>,

    /// How many, counted from the newest [default: 30, or 500 with --json]
    #[arg(short = 'n', value_name = "COUNT")]
    pub count: Option<i64>,

    /// Keep running and print new records as they happen
    #[arg(short = 'f', long, conflicts_with = "json")]
    pub follow: bool,

    /// Also show each step agents take inside their turns: what they say,
    /// the tools they use, and what comes back
    #[arg(short = 's', long)]
    pub steps: bool,

    /// Print JSON instead of text
    #[arg(long)]
    pub json: bool,
}
