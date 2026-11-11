# jarvis

One repository that is both the shared infrastructure and the home of the
work. `thoughts.md` says why; this says how.

## What there is

- **Processes.** A process is a name with its own directory of files, and an
  agent that can change them. Processes send each other messages. Its address
  is `name@kernel`, and so is its git branch.
- **Kernels** (`src/`): `jarvis up NAME` runs one. A kernel hosts processes
  on one machine: it keeps each one's service running, hands it its messages,
  commits what it changed, and writes down what happened. It also passes
  messages on to other kernels. It holds no opinion about what any process is
  for. Kernels are peers, there can be several on one machine, and you name
  each one yourself.
- **Links** connect kernels: `jarvis link`. A link is a local socket or any
  command whose input and output reach another kernel
  (`ssh host jarvis link --stdio`). Kernels pass on what is not for them, and
  each keeps a message until the next kernel has it.
- **The repository** holds all code, one branch per process. Every kernel of
  a site uses the same repository. While a process runs, its branch is
  checked out in a directory its kernel keeps.
- **Experts** are branches with plain names, such as `page`. An expert is a
  body of code together with what is known about it, never an agent itself.
  Its processes start from it (`jarvis spawn page0 --from page`), work on
  their own branches, and land finished work back on it by merge. A process
  whose code goes its own way creates a new expert branch from what it has
  become. All of this is plain git, described in `GUIDE.md`; kernels know
  nothing about experts.
- **The store** holds all records: every message, turn, commit and failure,
  as files that are written once and never changed. Each kernel has its own
  directory there (`STORE/<kernel>/`), and nothing in the store belongs to
  more than one kernel. A record is named by its kernel and number
  (`laptop:412`) and a file by the records it holds, so anyone can copy,
  merge and compact a store at any time without asking. What a kernel knows
  about other kernels, running or gone, it reads from there.
  `jarvis help store` has the details.
- **`GUIDE.md`**: what every agent is told about being a jarvis process. It
  holds the ideas; how to use the command is in the command's own help, which
  agents are shown too.
- **`harness/`**: one adapter per agent harness. `harness/claude` runs Claude
  Code and decides what its agents may do: by default, edit their own
  directory, use `jarvis`, use all of git, and search the web. A process adds
  the tools its own work needs (`npm`, `cargo`, `sbatch`) in
  `.jarvis/claude.json` in its directory, which is part of its expert. The
  adapter also reports each step its agent takes while the turn runs:
  `jarvis log -fs` shows them.
- **The page** is not here. It is an expert, the branch `page`: an Axum
  server with a Svelte and TypeScript page. Nothing on `main` knows about it.

## Starting

    cargo install --path .            # puts `jarvis` on your PATH
    jarvis up laptop                  # in this repository; leave it running in one
                                      # terminal. Ctrl-C stops it and everything it started

In another terminal, anywhere:

    jarvis spawn page0 --from page
    # first start builds the page (a minute or so), then: http://127.0.0.1:7878

    jarvis spawn polaris --prompt "You are our expert on the Polaris cluster."
    jarvis send polaris How do I get an interactive GPU node?
    jarvis spawn scratch   # blank: does nothing until it gets a message

    jarvis ps            # who exists and what each is doing
    jarvis ps polaris    # everything about one process
    jarvis messages      # what was said
    jarvis log -f        # what is happening
    jarvis watch         # the same as JSON lines, for programs: every change, once
    jarvis tag polaris project=moe   # say what it belongs to; the page groups by tags
    jarvis stop polaris  # end its turn and service now; its mail waits
    jarvis start polaris # let it go on
    jarvis remove scratch # take it off its kernel; what was written down stays

`jarvis` alone lists every command; `jarvis help <command>` explains one in
full, with examples. Processes use the same commands to talk to and create
each other.

On a fresh clone the branch `page` exists only on the remote so far;
`--from page` finds it there.

## Several kernels

    jarvis up node2                         # a second kernel, in another terminal
    jarvis --on laptop link node2           # connect them
    jarvis kernels                          # which are known, which can be reached
    jarvis spawn lammps --on node2 --from lammps
    jarvis send lammps@node2 Build with Kokkos, please.

With several kernels on a machine, say which one a command is for with
`--on NAME` or `$JARVIS_KERNEL`; a process's commands go to its own kernel.
A kernel's name is for good: it is part of the addresses and branches of
everything it hosts, so no other kernel may ever have it.

Where things are kept (`jarvis help up`):

| What | Default | On a cluster |
|---|---|---|
| The repository | the one `jarvis up` is run in | network storage |
| The store | `.jarvis/store` in it, or `$JARVIS_STORE` | network storage |
| A kernel's own state | `.jarvis/kernel/NAME` in it | the node's own disk |
| Sockets | `/tmp/jarvis-<uid>`, or `$JARVIS_RUN` | the node's own disk |

## A process's code

Two optional executables in a process's `.jarvis/` directory, in any language,
run from the top of its tree:

- `.jarvis/run`: a long-lived program, kept running while the process exists
  and restarted when a turn changes its files. If it fails, its agent is told.
- `.jarvis/handle`: run once per message. Exit 0: handled. Exit 3: give it to
  the agent. Anything else: failed, give it to the agent with the error.

With neither, the agent handles every message.

## How the kernel is built

- `kernel.rs`: one loop owns everything a kernel knows and is the only writer
  of its database. Commands, turns ending, lines from links and the clock all
  reach it as events. Nothing is polled.
- `index.rs`: records, and the database they add up to. `apply` is the one
  place that says what a record means.
- `store.rs`: writing records to the store, reading other kernels' from it,
  compacting, merging.
- `serve.rs`: carrying out commands. A command typed anywhere is sent to a
  kernel's socket as it was typed; `cli.rs` defines the commands and their help.
- `net.rs`: links. `git.rs`, `children.rs`: git, and the programs a kernel starts.

`cargo test` checks what records mean, and that stores merge and compact
without loss, also when several compact the same kernel at once. `sh tests/network.sh` starts three kernels on this machine,
kills the one in the middle while messages cross it, and checks that every
message arrived once (no agent is run; it takes under a minute).

The design rule: jarvis provides simple tools that do what they say, and
facts. It does not decide. What is hard (two writers, a deletion that must
spread, whether a silent kernel is dead) is settled by the agents talking to
each other.

## When things go wrong

- **A message is never dropped.** If a turn fails (an outage, a crash), its
  messages wait, whoever sent them is told once, and the kernel offers them
  again: after 5 seconds, then 10, doubling up to 5 minutes, for as long as
  it takes. `jarvis ps` shows such a process as `failing`. `jarvis stop` ends
  the trying; `jarvis start` tries again at once.
- **A message for another kernel waits on the way** while a link is down or a
  kernel is not running, and goes on when it can. `jarvis messages` shows how
  far each got. It arrives once, however often it had to be offered.
- **A message given a second time says so**, because the first turn may
  already have acted on it.
- **One process's problem stays its own.** The kernel writes it down and goes
  on with the others.
- **A kernel that was killed** leaves its processes' commits in the
  repository and its records in the store, up to the last few seconds
  (`--ship-every`). Started again under its name, it first ends whatever it
  left running, and carries on. If its own state is lost too, start a kernel
  under a new name and spawn its processes from their old branches.
- **Ending a program ends everything it started.**
- **An unfinished merge is never committed.** The agent is told at its next
  turn.

Nothing restarts a kernel if it dies; start it again with `jarvis up NAME`.
