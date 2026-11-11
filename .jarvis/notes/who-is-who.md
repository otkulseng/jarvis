# Who is who on the page

jarvis has many kernels, and the page is served by a process of one of them.
It shows the processes of every kernel that kernel knows of: its own at
once, the others' as their records reach the store (a few seconds later).

- **A process is known by its address**, `name@kernel` (`Proc.id`). That is
  what topics, routes and edges use. `Proc.name` and `Proc.kernel` are its two
  halves; `Proc.here` says whether it is on the page's own kernel.
- **For showing**, use `store.label(address)`: it gives the bare name for a
  process on the page's kernel, and the full address for one elsewhere.
- **The human is one.** jarvis says `human@laptop`, `human@node2`, depending
  on where they typed. The server turns every one of them into `human`
  (`canon` in `src/model.rs`) before a browser sees it. Likewise `kernel@X`
  becomes `kernel`.
- **A message's `id` is a string**, its name in jarvis (`laptop:412`): the
  kernel it was sent from and its number there. Messages are ordered by when
  they were sent (`at`), not by id. A fact's `id` is a number: its place in
  the feed of the page's kernel.
- **A message between kernels is brought twice**: by the fact that sent it,
  and by the fact that it arrived. `Model::apply` counts it, and shows it
  travelling, once (with the fact whose `id` is the message's `seq`).
- **Every process has the tag `kernel`**, so the map can be arranged by
  kernel like by any other tag.
- A message can be `sent` (not yet passed on) or `forwarded` (on its way
  through other kernels) before it is `pending` at its recipient.
