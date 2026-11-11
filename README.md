# The page

A small web server and a web page. This code is the expert `page`: its
processes work on their own branches and land finished work on the branch
`page`.

- `.jarvis/run`: builds the page (`npm ci`, then `svelte-check` and `vite build`),
  builds the server (`cargo build --release`), and starts it. If any step
  fails, it exits with an error and you are told what it printed.
- `src/main.rs`: the server (Rust, Axum). It serves `web/dist` and has two
  API routes, described at the top of the file. It gets everything by running
  the `jarvis` command and keeps no state.
- `web/`: the page (Svelte 5 with runes, TypeScript, Vite).
  - `src/App.svelte`: fetches a snapshot every 1.5 seconds and lays out the
    three panels.
  - `src/lib/api.ts`: the types of what the server returns, and the two calls.
  - `src/lib/Processes.svelte`, `Conversation.svelte`, `Facts.svelte`: the panels.
  - `src/lib/turns.ts`: groups an agent's steps into turns and pairs each tool
    use with its result. `Turn.svelte` draws one turn inside the conversation:
    folded to one line, or opened to show what the agent said and each tool
    use, which opens in turn to show its full input and output.
  - `src/app.css`: colours as CSS variables, with a dark variant. Use the
    variables instead of literal colours.

Changing a file here restarts `.jarvis/run`, which rebuilds everything; a type error
or a compile error stops the build and comes back to you as a message. New
npm packages need network access during `npm ci` and must be added to both
`package.json` and `package-lock.json`.
