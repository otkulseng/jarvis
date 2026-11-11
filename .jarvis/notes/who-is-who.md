# Who is who on the page

jarvis has many kernels, and the page is served by a process of one of them.
No kernel holds another kernel's state, so the page follows every kernel
that can be reached (`jarvis watch --all`) and asks each for its own
(`jarvis messages ... --all`, `jarvis ps --all`). Kernels that come into
reach later are followed from then on.

- **A process is known by its address**, `name@kernel` (`Proc.id`). That is
  what topics, routes and edges use, and what every `jarvis` command takes.
  `Proc.name` and `Proc.kernel` are its two halves.
- **For showing**, use `store.label(address)`: it gives the short name for a
  process on the page's kernel, and the full address for one elsewhere.
- **The human is one.** jarvis says `human@laptop`, `human@node2`, depending
  on where they typed. The server turns every one of them into `human`
  (`canon` in `src/model.rs`) before a browser sees it. Likewise `kernel@X`
  becomes `kernel`. What the human types on the page is sent as the human at
  the page's own kernel.
- **A message's `id` is a string**, its name in jarvis (`laptop:412`): the
  kernel it was sent from and its number there. Messages are ordered by when
  they were sent (`at`), not by id, and older ones are fetched by time.
- **A fact's `id` is a number**: its place in the order of the kernel it is
  from (`kernel` on every line of the feed). Facts about a process all come
  from that process's kernel.
- **A message between kernels is reported by both kernels**: by the fact
  that sent it and by the fact that it arrived, and each kernel says how far
  it got as it knows it. `Model::apply` counts it, and shows it travelling,
  once (the first time it is seen), and of two reports of its status the one
  that is further along counts (`rank`).
- **A kernel that cannot be reached any more** sends nothing; the feed says
  `lost`, and its processes stay on the map in the state `unreachable` until
  it is followed again.
- **Every process has the tag `kernel`**, so the map can be arranged by
  kernel like by any other tag.
- A message can be `sent` (not yet passed on) or `forwarded` (on its way
  through other kernels) before it is `pending` at its recipient. A request
  to a kernel that could not be carried out is `failed`.
