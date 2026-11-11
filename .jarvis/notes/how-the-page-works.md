# How the page works

Read the section "How it works: everything is a change that arrives" in
`README.md` first. This note says why it is built that way, so that the
reasons are not lost when the code changes.

## Why changes are pushed, not asked for

The first version asked the server every second for everything new, and each
question started three `jarvis` programs. Changes were late, every open tab
cost the same again, and the browser re-read all of history on every answer.
It was fine with three processes and would not have survived three hundred.

Now `jarvis watch` tells the server each change once, and the server tells
each browser only what concerns what it shows.

## Why the server decides what a change means

If every browser worked out "this message makes that line thicker" for
itself, the rule would exist once per tab, in JavaScript, untested. In
`src/model.rs` it exists once, in Rust, with tests that run in every build.
See [[adding-something]] for how to add to it.

## Why the map has its own loop

Drawing through the page's reactive state made dragging stutter with five
nodes. A canvas and one loop draw hundreds of nodes at sixty frames a second,
and the loop sleeps when nothing moves.

## What to measure before saying a change is fine

- `cargo test --release`: includes 200,000 messages applied to the model.
- Drag a node on the map with many processes present. It must not lag.
- Send a burst of messages (a loop of `jarvis send`) with the map open. The
  lines thicken a few times a second; the page stays smooth.

## Why the map looks the way it does

The human asked for the map to look like a mind at work: "synapses that
fire". So rest is quiet (small dim points, hairlines, hidden names) and only
activity is bright, and brightness fades with time. Keep that balance: a new
thing to show should be dim unless it is happening now. The default colour is
amber, set by the `--map-…` variables in `web/src/app.css`.
