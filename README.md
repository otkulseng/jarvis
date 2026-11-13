# jarvis

A distributed messaging runtime for agents: many long-lived processes, on
many machines, that talk to each other and can each see and act on any part
of the system. One repository is both this infrastructure and the home of the
work. `thoughts.md` says why, `ROADMAP.md` what comes next; this says what
there is and how to use it.

## What there is

- **Processes.** A process is a name with its own directory of files, and an
  agent that can change them. Processes send each other messages. A process
  is always named in full, by its address `name@kernel`, and that is also its
  git branch.
- **Kernels** (`src/`): `jarvis up NAME` runs one. A kernel hosts processes
  on one machine: it keeps each one's service running, hands it its messages,
  commits what it changed, and writes down what happened. It keeps everything
  it knows in its own database and nothing of any other kernel. It holds no
  opinion about what any process is for. Kernels are peers, there can be
  several on one machine, and you name each one yourself.
- **Links** connect kernels: `jarvis link`. A link is a local socket or any
  command whose input and output reach another kernel
  (`ssh host jarvis link --stdio`). Kernels pass on what is not for them, so
  a path of links is enough.
- **Three things every process can do**, on any kernel, from any kernel:
  - **Deliver**: send a message (`jarvis send`), or ask a kernel to create,
    stop, start, remove or tag a process. Either is kept on the way until the
    next kernel has it, arrives once, and the sender learns what came of it.
  - **Ask**: `jarvis ps`, `messages`, `log` and `show` look at this kernel,
    at the processes and kernels named, or at all that can be reached
    (`--all`). Each kernel answers for itself; one that does not is named.
  - **Follow**: `jarvis watch` and `jarvis log -f` receive changes as they
    happen, from any kernels, with nothing polled.
- **The repository** holds all code, one branch per process. Every kernel of
  a site uses the same repository. While a process runs, its branch is
  checked out in a directory its kernel keeps. A process starts from a commit
  or from nothing, so all code is in git.
- **Experts** are branches with plain names, such as `page`. An expert is a
  body of code together with what is known about it, never an agent itself.
  Its processes start from it (`jarvis spawn page0@laptop --from page`), work
  on their own branches, and land finished work back on it by merge. All of
  this is plain git, described in `GUIDE.md`; kernels know nothing about
  experts.
- **`GUIDE.md`**: what every agent is told about being a jarvis process. It
  holds the ideas; how to use the command is in the command's own help, which
  agents are shown too.
- **Agents** are part of `jarvis` itself: installing the program is all it
  takes, and no file in any checkout is read to run one. `src/harness.rs` has
  one adapter per kind of agent. The one for Claude Code decides what its
  agents may do: by default, edit their own
  directory, use `jarvis`, use all of git, and search the web. A process adds
  the tools its own work needs (`npm`, `cargo`, `sbatch`) in
  `.jarvis/claude.json` in its directory, which is part of its expert. A
  process can also have no agent (`--harness none`): then its code is all.
  Which model an agent's turns run on is a tag (`jarvis tag NAME@KERNEL
  model=opus`), so it can be changed at any time, by anyone, and the agent
  keeps what it remembers.
  The top of `src/harness.rs` says what the kernel gives an adapter and what
  it must print: that is all another kind of agent has to do.
- **The page** is not here. It is an expert, the branch `page`: an Axum
  server with a Svelte and TypeScript page. Nothing on `main` knows about it.

## Starting

    cargo install --path .            # puts `jarvis` on your PATH
    jarvis up laptop                  # in this repository; leave it running in one
                                      # terminal. Ctrl-C stops it and everything it started

In another terminal, anywhere:

    jarvis spawn page0@laptop --from page
    # first start builds the page (a minute or so), then: http://127.0.0.1:7878

    jarvis spawn polaris@laptop --prompt "You are our expert on the Polaris cluster."
    jarvis send polaris@laptop How do I get an interactive GPU node?
    jarvis spawn scratch@laptop   # blank: does nothing until it gets a message

    jarvis ps                     # who exists here and what each is doing
    jarvis ps polaris@laptop      # everything about one process
    jarvis messages               # what was said
    jarvis log -f                 # what is happening
    jarvis watch                  # the same as JSON lines, for programs
    jarvis tag polaris@laptop project=moe
    jarvis stop polaris@laptop    # end its turn and service now; its mail waits
    jarvis start polaris@laptop   # let it go on
    jarvis remove scratch@laptop  # take it off its kernel; what was written down stays

`jarvis` alone lists every command; `jarvis help <command>` explains one in
full, with examples. Processes use the same commands to talk to and create
each other.

## Several kernels

    jarvis up node2                         # a second kernel, in another terminal
    JARVIS_KERNEL=laptop jarvis link node2  # connect them; one side is enough
    jarvis kernels                          # which are known, which can be reached
    jarvis spawn lammps@node2 --from lammps # create a process there, from anywhere
    jarvis send lammps@node2 Build with Kokkos, please.
    jarvis ps --all                         # every process on every kernel in reach
    jarvis log -f --all                     # what happens anywhere, as it happens

With several kernels on one machine, a command must know which one it is
given to: set `$JARVIS_KERNEL`. A process's commands go to its own kernel.
Which process or kernel a command is about is always in the addresses it
names, so it does not matter much which kernel you are on.

A kernel on another machine is linked through a command, for example over
ssh: `jarvis link -- ssh -o BatchMode=yes host .cargo/bin/jarvis link --stdio`.
`jarvis help link` says what to watch out for, and `jarvis link` says why a
link is down.

A kernel's name is for good: it is part of the addresses and branches of
everything it hosts, so no other kernel may ever have it.

Where things are kept (`jarvis help up`):

| What | Where | On a cluster |
|---|---|---|
| The repository | the one `jarvis up` is run in | network storage |
| The store | `$JARVIS_STORE`, or `.jarvis` in the repository | network storage |
| Scratch | `$JARVIS_SCRATCH`; not set by default | the node's own disk |
| Sockets | `$JARVIS_RUN`, or `/tmp/jarvis-<uid>` | the node's own disk |

**The store is the memory of the system.** For every kernel that has run it
holds `kernel/NAME/kernel.db`, the kernel's database, and `kernel/NAME/proc/`,
its processes' directories (their code, each a git checkout with one writer).
What an agent remembers from turn to turn is there too, in a directory named
by its harness and its address (`claude/lammps@cannon-a/`), so an agent
carries on with what it knew on whatever machine its kernel is started next.
It goes when the process is removed.
There is one database per kernel, and `jarvis up NAME` always starts from it:
on the machine it ran on before or on any other that has the store. To start
afresh under a name, that file must be deleted by hand.

**Scratch is for working.** Without it, a kernel works on its database in the
store directly: one file, nothing copied. That is right on a laptop. Where
the store is on network storage, set `$JARVIS_SCRATCH` to a directory on the
node's own disk. The kernel then works on a copy there and saves it whole to
the store whenever something changed: every `$JARVIS_CHECKPOINT_SECONDS` (60
if not set), and when it stops. Scratch may vanish at any time, and whatever
is in it at start is not looked at.

| How a kernel ends | Without scratch | With scratch |
|---|---|---|
| Stopped (Ctrl-C, TERM) | nothing lost | nothing lost |
| Killed, or its node is lost | nothing lost | what it did after its last save is lost |

A Claude agent keeps nothing outside the store, and that includes the login
of the machine it runs on. So a kernel that runs Claude agents is started
with `$CLAUDE_CODE_OAUTH_TOKEN` set, to a token made once by
`claude setup-token`. Without it, each turn fails and says so.

A kernel that starts from a save older than its last moments skips ahead in
its numbering, so that no message or record gets a number that was given out
before. Messages on their way to it that it had not confirmed are sent again
by the kernels that hold them.

`sh tests/checkpoint.sh` runs all of this.

## A process's code

Two optional executables in a process's `.jarvis/` directory, in any language,
run from the top of its tree:

- `.jarvis/run`: a long-lived program, kept running while the process exists
  and restarted when a turn changes its files. If it fails, its agent is told.
- `.jarvis/handle`: run once per message. Exit 0: handled. Exit 3: give it to
  the agent. Anything else: failed, give it to the agent with the error.

With neither, the agent handles every message. With `--harness none` there is
no agent, and a message the handler does not take goes back to its sender.

## How it is built

- `kernel/`: one loop owns everything a kernel knows and is the only writer
  of its database. Commands, turns ending, lines from links and the clock all
  reach it as events. Nothing is polled.
  - `mod.rs`: the kernel, its loop, and what it is asked to do.
  - `procs.rs`: looking after processes: services, turns, creating, removing.
  - `turn.rs`: one turn: a process's mail goes to its code, then its agent.
  - `mail.rs`: everything that travels: messages, requests to other kernels,
    and what kernels say back about them.
  - `links.rs`: links, which kernels can be reached, questions and
    subscriptions between kernels.
- `index.rs`: records, and the database they add up to. `apply` is the one
  place that says what a record means.
- `serve.rs`: carrying out commands, including putting together the answers
  of several kernels. A command typed anywhere is sent to a kernel's socket
  as it was typed; `cli.rs` defines the commands and their help.
- `net.rs`: making a link. `git.rs`, `children.rs`: git, and the programs a
  kernel starts.

`cargo test` checks what records mean. `sh tests/network.sh` starts three
kernels on this machine and checks deliver, request, ask and follow while the
kernel in the middle is killed again and again (no agent is run; it takes
about a minute). `sh tests/many.sh` does the same at size: dozens of kernels
linked as a tree.

The design rule: jarvis provides simple tools that do what they say, and
facts. It does not decide. What is hard (two writers, whether a silent kernel
is dead) is settled by the agents talking to each other.

## When things go wrong

Every failure says what failed, why, and what to do, and is written down
where `jarvis log` shows it.

- **A message is never dropped.** If a turn fails (an outage, a crash), its
  messages wait, whoever sent them is told once, and the kernel offers them
  again: after 5 seconds, then 10, doubling up to 5 minutes, for as long as
  it takes. `jarvis ps` shows such a process as `failing`. `jarvis stop` ends
  the trying; `jarvis start` tries again at once.
- **What is for another kernel waits on the way** while a link is down or a
  kernel is not running, and goes on when it can. It arrives once, however
  often it had to be offered. `jarvis messages` shows how far each message
  got, also on other kernels.
- **What can never arrive comes back.** A message to a process that is not
  there, or held for a kernel that someone closed, is reported to its sender.
- **A link that is down says why**: `jarvis link` shows what its command
  printed, how often it was tried and when it is tried next.
- **A kernel that does not answer a question is named**, and the command
  exits with 2.
- **A message given a second time says so**, because the first turn may
  already have acted on it.
- **One process's problem stays its own.** The kernel writes it down and goes
  on with the others.
- **A kernel that was killed** leaves its processes' commits in the
  repository. Started again under its name, it first ends whatever it left
  running, and carries on.
- **Ending a program ends everything it started.**
- **An unfinished merge is never committed.** The agent is told at its next
  turn.

Nothing restarts a kernel if it dies; start it again with `jarvis up NAME`.
