# The page

The web page of jarvis: a map of every process, the conversation with each,
their files and notes, and an inbox. This code is the expert `page`: its
processes work on their own branches and land finished work on the branch
`page`.

## What the reader sees

- **The map** (`#/`): every process is a node, coloured by the expert it was
  started from and drawn by its state (idle, working, failing, stopped).
  A line joins two that have exchanged messages, thicker the more they have,
  dashed and moving while a message on it is waiting. A new message travels
  along its line. Clicking a node opens that process; clicking a line opens
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
    cargo build --release          # build the server
    curl -s localhost:$(cat .build/port)/api/snapshot | head -c 300

## The code

- `src/main.rs`: the server (Rust, Axum). It serves the built page and has a
  few API routes, described at the top of the file. It gets everything by
  running the `jarvis` command and keeps no state.
- `web/`: the page (Svelte 5 with runes, TypeScript, Vite). In `web/src`:
  - `app.css`: colours and sizes as CSS variables, light and dark. Use the
    variables, never literal colours.
  - `App.svelte`: the frame: the bar on top, the view in the middle, the dock.
  - `lib/store.svelte.ts`: everything the page knows (processes, messages,
    facts, steps), fetched every second or so; where the reader is (the
    address after `#`); what they have read.
  - `lib/api.ts`: the types of what the server returns, and the calls.
  - `lib/MapView.svelte`, `lib/graph.ts`, `lib/colours.ts`: the map, where
    its nodes go, and their colours.
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
