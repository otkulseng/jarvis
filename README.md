# The page

The web page of jarvis: a map of every process, the conversation with each,
their files and notes, and an inbox. This code is the expert `page`: its
processes work on their own branches and land finished work on the branch
`page`.

## What the reader sees

- **The map** (`#/`): every process is a small point, and it is meant to
  look like something thinking. At rest everything is dim and quiet. A
  message is a bright head with a tail running along its line; the line stays
  lit behind it and cools over a few seconds; the process it reaches flares.
  A process that is working glows and breathes. Failing is red, stopped is a
  hollow ring, a small white dot means it wrote to the reader. Names appear
  when zoomed in or pointed at. A point that is dragged stays where it was
  put, here and in every other tab; double-click frees it.
  A line joins two that have exchanged messages, and glows faintly while a
  message on it is waiting. Clicking a node opens that process; clicking a line opens
  the messages between its two ends.
- **A process** (`#/p/NAME`): its conversation with the reader, with each of
  its turns folded between the messages; its files and notes, with markdown
  drawn as pages and `[[links]]` between notes followed; and what the kernel
  recorded about it. Stop, Start and Remove are buttons here. Creating a
  process is not: the reader asks an agent for that.
- **A line** (`#/e/A/B`): the messages between two processes, to read along.
- **The inbox** (`#/inbox`): everything addressed to the reader.
- **The dock** (right, ⌘J): the conversation with the process serving the
  page, available everywhere. A message sent from it ends with
  `[page: looking at …]`, so that "this looks wrong" says what "this" is.

## How it is served, and how a change reaches the reader

`.jarvis/run` is started by the kernel, and again whenever a turn changed the
files here. It never leaves the reader without a page:

1. If only notes changed, the build already there is served again at once.
2. Otherwise the last good build is served while the new one is built.
3. When the build succeeds the new one takes over (a fraction of a second)
   and every open page reloads itself.
4. When it fails, the old build stays up and you are sent the error.

Builds live in `.build/` (not committed), with the port this process serves
on in `.build/port`. The first page process on a machine gets 7878 and keeps
it; every other one gets its own, so several processes of this expert can
each show their own version. `cat .build/port` says which is yours.

**Check your work before you end a turn**, since ending it is what puts it in
front of the reader:

    (cd web && npm run build)      # type check and build the page
    cargo test --release           # what each change means, and that many stay cheap
    cargo build --release          # build the server
    curl -s -N -m 2 localhost:$(cat .build/port)/api/stream | head -c 400

## How it works: everything is a change that arrives

jarvis is many processes doing things at times nobody can predict. The page
is built the same way: nothing asks "what is new?" on a timer. Changes arrive,
and each kind of change is dealt with in one small place.

    jarvis watch ─▶ src/model.rs ─▶ src/hub.rs ─▶ browser: live/feed.ts ─▶ live/store.svelte.ts ─▶ views
    (every change)  (what it means)  (batches,     (one stream,            (one handler per
                                      per topic)    once per frame)         kind of frame)

- **The server is the one place that understands jarvis.** It follows
  `jarvis watch`, keeps the current picture (who exists, how much each two
  have written to each other), and turns each change into *frames* for the
  browsers. `src/model.rs` lists the frames and the topics.
- **A browser is told only about what it is showing.** It asks for topics
  (`map` and `inbox` always; `process:NAME` or `edge:A|B` for what is open)
  and gets the frames of those.
- **Bursts are flattened twice.** The server sends what changed a few times a
  second, and of two states of the same thing only the newer. The browser
  applies what arrived once per drawn frame.
- **The browser keeps what is on screen, not history.** Earlier messages and
  the steps of earlier turns are fetched when the reader asks for them.
- **The map is drawn by its own loop** (`web/src/lib/map/engine.ts`), on a
  canvas, outside the page's reactive state.

Four rules keep it that way. Break one and the page gets slow or wrong under
load, in ways that do not show with three processes:

1. Only `live/feed.ts` listens to the network for what is new. Views never
   fetch on a timer.
2. What a change *means* is decided in `src/model.rs`, once, and tested there.
   The browser does not work it out again.
3. Nothing is recomputed over everything that ever happened. Keep counters;
   fetch history on demand.
4. Code that runs every frame (`map/engine.ts`) never reads or writes the
   page's reactive state, and knows nothing about jarvis.

`.jarvis/notes/adding-something.md` is the recipe for the common additions.

## The code

- `src/`: the server (Rust, Axum). The routes are listed at the top of
  `main.rs`.
  - `jarvis.rs`: running the `jarvis` command, and following `jarvis watch`.
    The page knows jarvis only through that command.
  - `model.rs`: the picture, and `apply`: one arm per kind of change. No
    input or output, so it is all tested (`cargo test`).
  - `hub.rs`: gathers what changed and sends it to each browser in batches.
    Lets go of a browser that cannot keep up (it reconnects and starts fresh).
  - `threads.rs`: the history of a topic, fetched from jarvis when opened.
- `web/`: the page (Svelte 5 with runes, TypeScript, Vite). In `web/src`:
  - `app.css`: colours and sizes as CSS variables, light and dark. Use the
    variables, never literal colours.
  - `App.svelte`: the frame: the bar on top, the view in the middle, the dock.
  - `lib/live/feed.ts`: the one stream from the server.
  - `lib/live/store.svelte.ts`: everything the page knows, with one handler
    per kind of frame; where the reader is (the address after `#`); what they
    have read.
  - `lib/api.ts`: the types, and the calls the reader's actions make.
  - `lib/map/engine.ts`: the map on a canvas: layout (d3-force), drawing,
    pointer. `lib/map/MapView.svelte` tells it what to draw and draws what
    sits on top. Its colours are the `--map-…` variables in `app.css`.
  - `lib/ProcessView.svelte`, `lib/Notes.svelte`, `lib/Activity.svelte`:
    inside one process.
  - `lib/EdgeView.svelte`, `lib/Inbox.svelte`, `lib/Dock.svelte`: the other
    views.
  - `lib/Thread.svelte`, `lib/Turn.svelte`, `lib/turns.ts`,
    `lib/Composer.svelte`: a conversation: messages, the agent's turns
    between them (each tool use paired with its result), and the box to type
    in.
  - `lib/markdown.ts`: the markdown that agents write, as safe HTML.

New npm packages need network access when they are installed and must be in
both `package.json` and `package-lock.json`.
