# nary is not a compiler (stated by the human, 2026-11-13)

Keep this in mind before proposing or making any change.

- nary is **not a compiler**. It does not optimize, lower, schedule, fuse or
  generate code. Kernels are written by people or agents; nary only chooses
  among them.
- egglog is used for **one thing only: mapping subgraphs onto kernels**. The
  equivalences say which expressions are the same across dialects; the
  e-graph finds every form of the graph; extraction picks one in which every
  symbol has a kernel. Nothing else is to be done with egglog (no algebraic
  simplification, no rewrite-based optimization passes).
- The default extraction is **"the shortest graph"** (fewest symbols). A
  possible future is to estimate latencies instead. Cost modelling is not the
  point for now.
- What the e-graph is mainly for:
  a) **proving there is a valid extraction**: some form of the graph has a
     kernel for every symbol, or the mapping fails with the symbols missing;
  b) **derivatives, later**: the derivative through a node can be taken
     through the node itself (if it has one), or through anything it is
     equivalent to. The e-graph already holds those alternatives.

Do not suggest compiler-like features (optimization passes, IR lowering,
intra-dialect rewrite rules, backends) as if they were the direction. The
rule that an equivalence must span two dialects follows from this.
