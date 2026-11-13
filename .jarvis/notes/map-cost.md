# Equivalences go to egglog as DAGs, never as nested expressions

Found and fixed 2026-11-13. Keep it this way.

An equivalence is a DAG in nary (nodes are hash-consed). `map` first wrote
each side as one nested egglog expression, so a node used twice was written
twice, with everything under it. For n Euler steps
`add(p, mul(dt, div(neg(p), m)))`, where `p` is used twice per step, the
rule doubled with each step:

    steps  DAG nodes  map (before)
      6       25        9 ms
      8       33       35 ms
     10       41      306 ms
     12       49      4.58 s

The time was egglog taking in the huge rule (310-340 ms at 10 steps);
saturation was 5.7 ms and extraction 0.2 ms. It looked like "egglog is slow"
and was not.

Now `side()` in `src/map.rs` names each node of a side once (via `graph()`,
which is topological and iterative): the searched side is one `Fact::Eq` per
node, the added side one `Action::Let` per node, then a union of the two
roots. `dialects_in()` in `dialect.rs` walks the same way. A 40-step
equivalence is a test (`tests/dialect.rs`).

After: lj tests 0.57 s -> 0.06 s; argon 2.10 -> 1.53 ms a step, same numbers.

Still a tree walk: `written()` in `dialect.rs`, used to print a dialect and
in the message for an equivalence within one dialect. Printing a dialect
with a deeply shared equivalence is exponentially long. Left as is: how a
large equivalence should be printed is the human's to say.
