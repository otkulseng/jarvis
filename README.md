# jarvis

One repository that is both the shared infrastructure and the home of the
work. `thoughts.md` says why; this says how.

## What is here

- **The kernel** (`src/`): a single small program, `jarvis`. `jarvis up` runs
  it. It keeps each process's service running, hands messages to processes,
  commits what they change, and records what happened. It holds no opinion
  about what any process is for.
- **Processes** are branches of this repository, named `jarvis/<id>`. A
  process is an identity with a mailbox, its own directory of files, and an
  agent that can change them. While a process runs here its branch is checked
  out under `.jarvis/proc/<id>/`. The id is random and long enough to be
  unique on any machine; where a process runs is recorded, not part of its
  name.
- **A process's tree** holds the work itself (code, data, documents) and one
  directory, `.jarvis/`, with everything that is about the process: its
  prompt, its description, its notes, its two optional executables. Delete
  that directory and what is left is an ordinary repository.
- **The registry** (`.jarvis/jarvis.db`, not in git): who exists, the
  messages, what the kernel has running, and the log of facts.
- **`GUIDE.md`**: what every agent is told about being a jarvis process. It
  holds the ideas; how to use the command is in the command's own help, which
  agents are shown too.
- **`harness/`**: one adapter per agent harness. `harness/claude` runs Claude
  Code and decides what its agents may do: by default, edit their own
  directory, use `jarvis`, use all of git, and search the web.
- **The page** is not here. It is a branch of its own, `tree/page`: an Axum
  server with a Svelte and TypeScript page, which a process can be started
  from. Nothing on `main` knows about it.

## Starting

    cargo install --path .            # puts `jarvis` on your PATH
    jarvis init                       # once, in this repository
    jarvis up                         # leave running in one terminal; Ctrl-C stops
                                      # it and everything it started

In another terminal:

    jarvis spawn page --from tree/page
    # first start builds the page (a minute or so), then: http://127.0.0.1:7878

    jarvis spawn polaris --prompt "You are our expert on the Polaris cluster."
    jarvis send polaris How do I get an interactive GPU node?
    jarvis spawn scratch   # blank: does nothing until it gets a message

    jarvis ps            # who exists and what each is doing
    jarvis ps polaris    # everything about one process
    jarvis messages      # what was said
    jarvis log -f        # what is happening
    jarvis stop polaris  # end its turn and service now; its mail waits
    jarvis start polaris # let it go on

`jarvis` alone lists every command; `jarvis help <command>` explains one in
full, with examples. Processes use the same commands to talk to and create
each other.

On a fresh clone the page's branch is `origin/tree/page`:
`jarvis spawn page --from origin/tree/page`.

## A process's code

Two optional executables in a process's `.jarvis/` directory, in any language,
run from the top of its tree:

- `.jarvis/run`: a long-lived program, kept running while the process exists
  and restarted when a turn changes its files. If it fails, its agent is told.
- `.jarvis/handle`: run once per message. Exit 0: handled. Exit 3: give it to
  the agent. Anything else: failed, give it to the agent with the error.

With neither, the agent handles every message.

## Several processes, one code

A code is a process. Several processes working on the same code are forks of
it (`jarvis spawn --from`), each on its own branch in its own directory, and
they exchange work with ordinary git merges. Code from elsewhere (LAMMPS,
PyTorch) is a git submodule in the tree of the process that is the expert on
it. `GUIDE.md` tells the agents how.

## When things go wrong

- **A message is never dropped.** If a turn fails (an outage, a crash), its
  messages stay in the mailbox, whoever sent them is told once, and the kernel
  offers them again: after 5 seconds, then 10, doubling up to 5 minutes, for
  as long as it takes. `jarvis ps` shows such a process as `failing`.
  `jarvis stop` ends the trying; `jarvis start` tries again at once.
- **A message given a second time says so**, because the first turn may
  already have acted on it.
- **One process's problem stays its own.** The kernel records it and goes on
  with the others.
- **Only one kernel runs per home**, and a kernel that starts after another
  died first ends whatever that one left running.
- **Ending a program ends everything it started.**
- **An unfinished merge is never committed.** The agent is told at its next
  turn.

Nothing restarts the kernel itself if it dies; start it again with `jarvis up`.
