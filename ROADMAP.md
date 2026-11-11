# Roadmap

What jarvis is meant to become and what is decided but not built. `thoughts.md`
says why jarvis exists, `README.md` what there is now. This file holds the
parts in between: agreed direction, in the order it will be done.

## What jarvis is

A distributed messaging runtime for agents. Processes anywhere on a network
of kernels talk to each other, robustly and at any scale, and any of them can
see and act on any part of the system. No kernel and no process is special.

It is built in three parts that do not depend on each other downwards: the
runtime is complete without the other two.

## Part 1: The runtime (being built)

A kernel hosts processes and keeps everything it knows in its own database.
Kernels are connected by links. On top of that, every process can:

- **Deliver**: send a message or a request (create, stop, start, remove, tag)
  to any address. It is carried hop by hop, held during outages, and carried
  out once. The sender learns what came of it.
- **Ask**: put a question (which processes, what happened, what was said) to
  one kernel, to several, or to all that can be reached. Each kernel answers
  from its own data, and the answer says who did not reply.
- **Follow**: receive changes as they happen, for what was named. Nothing
  polls.

Information is pulled by whoever wants it. Nothing is copied to every kernel
in advance.

On a machine whose disk does not vanish (a laptop, a virtual machine), the
runtime is all that is needed.

## Part 2: Checkpointing (designed next, not built)

**What it is for.** On a cluster, a kernel runs on a compute node whose disk
is gone when the job ends or the node fails. Checkpointing saves what the
kernel knows to storage that outlives the node.

**The goal.** A kernel whose disk is lost can be started again, somewhere
else, under its own name, and carry on with its processes and its waiting
mail. The rest of the system notices only a pause. Today such a kernel's name
is spent and its processes are carried on by hand from their branches.

**What it is not.** It is not how kernels learn about each other, and the
runtime never reads checkpoints to answer questions. (An earlier version had
kernels write logs to a shared directory and read each other's logs from it.
That mixed backup with visibility, made kernels poll the file system, and
could not work between sites. It was removed.)

**It is optional and per kernel**: switched on where a disk can vanish.

**The hard point, to be solved in the design.** A kernel confirms to its
neighbour that it has a message, and the neighbour then forgets it. If the
kernel dies after confirming and before its next checkpoint, that message is
lost. So with checkpointing on, a kernel may confirm only what is already in
a checkpoint. The question is how to do that without slowing every message
to the speed of network storage.

**What can be reused.** The file handling of the removed log store (git
history, `src/store.rs` at 429fc29): a kernel's records are numbered without
gaps, files are named by the records they hold and never changed, and anyone
can compact them at any time with no locks, by file names alone:
write `records-N.db`, give it its name only if the name is free, delete what
it covers.

**Also to decide then**: what besides records belongs in a checkpoint
(waiting mail, messages on their way, the steps agents take inside turns),
and how often it is taken, which is a tuning question.

## Part 3: Big file storage (later)

A content-addressed store, so that messages and records refer to large data
(results, datasets, long outputs) instead of carrying it. Optional, and
separate from the other two parts. It also keeps a kernel's database small
on any machine.

## Further out

- **Messages during a turn.** Today mail that arrives while an agent works
  waits until its turn ends. Claude Code can be given text between tool calls
  through hooks (a hook after each tool call that adds context, and a hook
  that keeps a turn going when it is about to stop). That is the documented
  way that fits the adapter as it is.
- **Usage.** What the agents cost and how much is left, across the system.
- **Real clusters** (Cannon, Polaris, Aurora): only when the human says so.
  The expert `lima` (the Lima virtual machine) is the rehearsal.
