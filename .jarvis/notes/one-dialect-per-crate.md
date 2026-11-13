# One dialect per crate, no `meaning()` (the human, 2026-11-13)

A dialect crate exposes a single `dialect()`: its equivalences and its
kernels together. Do not add a second constructor that holds only the
equivalences (`md::meaning()` was added for checking kernels and running on
np alone, and the human had it removed: "Stop adding 'meaning'. I don't
care. Remove it from the codebase. We have a single dialect. That is it.").

To check a kernel, write the formula in np in the test and compute it with
`[np::dialect()]` alone.

More generally: do not add API or options the human did not ask for (the
`np` mode of the argon example went with it).

Not settled when this was written: `tests/dialect.rs` has a `toy::meaning()`
that was in the human's own commit, and the README uses the word "meaning"
for what a symbol equals. Asked the human; see the messages.
