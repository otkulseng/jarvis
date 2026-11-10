# JARVIS
Quite obviously named after the movie character. I am going to use `jarvis` to assist myself in *high-performance computing code generation*. It must work at arbitrary scale (and here I really strive for arbitrary), which in my opinion can only occur by combining two things:

1. An *actor model* approach to concurrency: `jarvis` is a large assortment of **jarvis processes**.
2. Each process is heterogeneous, working robustly like a state machine (with state possibly in an `sqlite` database).

All of it implemented in an extremely simple, elegant, and robust fashion, under one central mantra:

> Extend the fail-early approach to fault tolerance, as in Erlang and other critical infrastructure, by allowing AI agents to handle the edge cases.

So: keep the infrastructure easy to use, fast, and able to handle the regular cases, and let agents handle the rest. A simple idea, but quite powerful, and even more so as agents become better. In the long run `jarvis` may get its own harness around an LLM API, but for now I much prefer the benefits of the massive development done by industry.

# One jarvis, many processes
`jarvis` is not for others: all `jarvis` processes in the world are the one and only `jarvis`. They orchestrate themselves into hierarchies and feedback loops of different time scales.

This removes context management, chat management, and so on. Through a single interface I can at all times talk to any process that is alive (in some sense of the word!), but I never have access to one other than through messages.

I do not care for a 'globally correct state'; if an application needs it, we spawn postgres or something within some nodes. What I need is to robustly orchestrate a large number of processes, so I can asynchronously work on multiple parts of the system.

# Software Over-the-Air
The processes must communicate in vastly different circumstances: on one node, across machines, web servers, and so on. Currently I have no earthly idea of how to achieve this generality.

A natural consequence is long-livingness. `jarvis` **must** be able to access its own code, in a structured manner, to allow for arbitrary improvement while remaining in communication with the other processes.

I am essentially betting that the agents we use do **not** reach recursive self-improvement, at which point it is wise to shut down `jarvis`. Until then, I will make the infrastructure that allows human knowledge to be recursively **refined**: the ideal scientific lab book and helper.

# Visuals from the get go
I thoroughly need visuals, from the get go, and I am not sure what shape they should take! What I do know:

1. The visuals are independent of the `jarvis` binary code. The webpage is its own code, and sees `jarvis` the same way I do: through messages.
2. The agents must be able to update the visual code arbitrarily, to show me what I want. Not merely new data on the page, but a new page if necessary.

My main devops feel will be this: I spawn a `jarvis` process here that can talk to all of my clusters and communicate with the `jarvis` processes here. It gives the local one the drawing board, so the local webpage is updated based on the data, and the code for the webpage itself is changed when the data calls for it.

# Xtreme Programming all the way, but no scope too big
The most important message of them all. `jarvis` improves by use and through communication with me. You do not have time to read all the books in the world, so you had better choose what to read with care. That is why XP and a big scope coexist:

1. **The problem at hand.** Needing to solve a problem, and solving it, is the XP thing. No hacky, temporary solutions: it is solved to 'space shuttle' rigor (i.e. real-time programming solutions are a must), because past a critical mass of code `jarvis` will never be turned off. It is MEANT to be kept alive, jumping from process to process, computer to computer.
2. **The guiding light.** The most IDEAL solution is worked on in the background. It improves over time and can always be put on the back burner (to not burn tokens), but it IS THE GUIDING LIGHT for the XP agents. We need the guide, even if we do not reach it.

# Why not use an agentic harness
Agents are extremely good at targeted tasks, and `jarvis` will use them for exactly that. But a harness should not be the structure of the system itself. My view (may be faulty logic):

1. **Actors** (human or LLM, able to generate code and **act**) are fallible stochastic machines.
2. Over a long sequence of decisions, reaching a suboptimal solution is almost guaranteed.
3. The only way to compensate is through ensembles (this is what humans do through peer review).
4. Better agents reduce the size of the ensemble necessary, but imo it never reaches 1.
5. Hence parallel development, exploration, collaboration, and critique: the principles of XP are more important than ever.

The other reason: *self-improvement warrants a feedback loop*, and scientific feedback loops are difficult to model within existing harnesses. With a database-backed state machine, an actor can fuse itself with another, perform fission (subdivide into identical or different agents), and communicate state. This gives *lineage* and *reproducibility*: **not** as strict as regular science, but strict enough that knowledge, system state, results, and artifacts can be traced and reasoned about.

It also unifies my earlier attempts at an agentic workflow engine: 'scientific workflows' fit in the same structure, so we can spawn a `jarvis` process on a cluster and teach it to use `flux` and `pixi` to run simulations reproducibly, scalably, and fast.
