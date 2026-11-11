# Adding something to the page

The page reacts to changes. Almost every addition is one of these four.

## Show something that jarvis already reports

`jarvis watch` prints a line for it (try `jarvis watch --steps` in a terminal
and make it happen). Then:

1. `src/model.rs`, in `apply`: add or extend the arm for that kind of line.
   Decide which *topics* are told (who needs to know) and whether newer
   frames replace older ones (`key`). Add a test next to the others.
2. `web/src/lib/live/store.svelte.ts`, in `handlers`: add a handler with the
   frame's `type` as its name. It changes `store.work` and marks what changed.
3. The view reads it from `store`.

A frame type without a handler is ignored, so the server side can go first.

## Show something jarvis does not report yet

That is a change to jarvis itself (`jarvis watch`), not to the page. Ask the
human, or whoever looks after jarvis.

## A new view

Add a route in `store.svelte.ts` (`Route`, `parse`, `href`), a component, and
its case in `App.svelte`. If it needs live data of its own, give it a topic:
`wanted()` in the store says which topics the page asks for, and
`src/threads.rs` says what a topic's history is.

## Something on the map

What is drawn and how: `web/src/lib/map/engine.ts`. Which nodes and lines
there are, their sizes and colours: `web/src/lib/map/MapView.svelte`. The
engine must stay ignorant of jarvis: give it numbers, colours and labels.

## Another way to group or find processes

Do not build one. The map arranges and lights by *tags*, and anything a
process belongs to can be a tag: `jarvis tag NAME kind=value`. A fact that
jarvis already reports about a process becomes a tag in one place, `tagsOf`
in `web/src/lib/live/store.svelte.ts`. `MapView.svelte` then offers it under
"arrange by" and "light" without further work.

See [[how-the-page-works]] for the reasons behind these places.
