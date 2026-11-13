# Why `map` is slow with large equivalences (found 2026-11-13, not fixed)

It is not egglog and not equality saturation. It is `pattern()` in
`src/map.rs`: it writes each side of an equivalence as one nested
expression, so a node that is used twice is written out twice, and so is
everything under it. An equivalence is a DAG in nary (nodes are
hash-consed), and becomes a tree for egglog.

Measured (release): n Euler steps `add(p, mul(dt, div(neg(p), m)))`, where
`p` is used twice per step, so the tree doubles with each step:

    steps  DAG nodes  map
      6       25        9 ms
      8       33       35 ms
     10       41      306 ms
     11       45      1.16 s
     12       49      4.58 s

The same physics written with `p` used once per step: 0.4-0.6 ms at every
size. Inside `map`, the time is in handing the rule to egglog (310-340 ms at
10 steps); saturation was 5.7 ms and extraction 0.2 ms.

The LJ equivalences share subterms too (`apart`, `inv2`, `inv6`), which is
their ~57 ms per `get`.

The fix, proposed to the human, who asked for discussion only: give egglog
each distinct node of a pattern once. On the side that is searched for, one
fact per node, bound to a variable; on the side that is added, one `let` per
node. Same meaning, cost linear in the size of the DAG.

The same tree-walking is in `dialect.rs`: `written()` (so printing a dialect
with such an equivalence is exponentially long) and `dialects_in()`.
