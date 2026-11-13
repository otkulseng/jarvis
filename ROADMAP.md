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

## Part 2: The store (built, except as said below)

**What it is.** `JARVIS_STORE` is the memory of the system. It holds the
database of every kernel that has run, the code of its processes, and what
their agents remember from turn to turn (found by address alone). A
kernel has one database, the one in the store, and always starts from it.
This is never optional: it is also where the data is when a run is over.

**Scratch.** A kernel's database must be on the machine's own disk to be
fast. Where the store is on network storage, `JARVIS_SCRATCH` names such a
disk: the kernel works on a copy there and saves it whole to the store when
something changed, at an interval and when it stops. On a laptop it is not
set, and the kernel works in the store directly.

**What may be lost.** A kernel with scratch that is killed loses what it did
after its last save. That is accepted: messages are carried by flux and the
like and need not outlive everything, the agents' work is in git, and their
conversations are in the store.

**It is not how kernels learn about each other.** The runtime never reads
another kernel's database to answer a question.

**Not built yet:**

- **Reading a kernel that is not running.** Its database is in the store, and
  no command reads it there.
- **One kernel name in two places.** Nothing stops a kernel being started on
  a second machine while it still runs on the first. Each machine guards its
  own. With one kernel per flux broker, flux does.
- **A database too big to copy whole.** Agents cannot make it so (about 55 KB
  per dollar of their work). Messages sent by code at a high rate can.

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
