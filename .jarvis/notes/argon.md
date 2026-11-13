# The argon example and what it needed (2026-11-13)

The human asked for a non-contrived example to start doing science with, and
agreed to liquid argon (Rahman 1964). Constraints the human set:

- **No derivatives for now**: "I am afraid of you implementing derivatives".
  Forces are encoded by hand as their own symbol (`md.lj_forces`), with no
  concept of a derivative anywhere. Do not add one unasked.

What was added:

- core: `Graph.results`, `graph_all`, `get_all`; `run` returns one
  `Rc<dyn Any>` per result and frees a step's result after its last use.
  `get`/`get_all` need `V: Clone`. In `map`, each root is extracted from the
  same cost table into one `TermDag`, so shared subterms become one step.
- np: NumPy broadcasting for the binary ops (`kernels/each.rs`), `round`,
  `less` (1.0/0.0), `sum(x, axis)`, `expand_dims(x, axis)`, and
  `array(Value)`: a leaf whose attribute is an index into a thread-local list
  of values (never freed).
- md: `lj_forces`, `lj_energy` (`md/src/lj.rs`: symbols and their np
  meaning; shifted, truncated LJ, minimum image), cell-list kernels sharing
  `kernels/pairs.rs`; `md::meaning()` = equivalences only, for checking.
- No attribute may depend on the number of atoms, since attributes are
  literal in an equivalence. Hence `expand_dims` instead of reshape, and box
  side / cutoff are passed as `np.const` nodes, not attributes.

Measured (release, macmini, 864 atoms):

- `run`: about 0.9 ms a step with the cell-list kernel.
- `map`: a fixed ~370 ms per call, independent of graph size (12 or 126
  steps). ~310 ms of it is the `md.euler_10_steps` equivalence (a 40-symbol
  pattern), ~57 ms the two LJ equivalences. It is egglog preparing the rules,
  every call. This is why the example uses 100 steps per `get`.
  Proposed to the human, not done: build the e-graph with its rules once per
  set of dialects and reuse it between calls.
- kernels vs np meaning agree to 1e-10 on forces and energy (tests/lj.rs).
  Whole trajectories agree to 5 digits for ~1000 steps and then drift apart:
  summation order differs and MD is chaotic. Averages agree.

Not done: pressure (needs the virial, another symbol), a kernel for a whole
Verlet step (needs a symbol with several results), units other than reduced.
