# How the code is laid out (after the rewrite of 2026-11-13)

Core crate `nary` (src/, ~750 lines), knows nothing about arrays:

- `lib.rs`: `Node(u32)` into a thread-local hash-consed `Store` of
  `Applied {symbol, attrs: String, inputs}`. `symbol` / `symbol_with` make
  nodes. Symbols are named `dialect.name`; attrs are free text. The store is
  per thread and never shrinks.
- `dialect.rs`: `Dialect` value = equivalences (`.equal(|[a,b,..]| (l, r))`,
  over `?` ANY nodes) + kernels (`.kernel` / `.kernel_with`, type-erased to
  `Fn(&str, &[&dyn Any]) -> Result<Box<dyn Any>, String>`). Mistakes are
  collected in `problems` and returned by map/run, not panicked on. An
  equivalence must span two dialects; a kernel must be for the dialect's own
  symbol.
- `graph.rs`: `graph(node)` -> `Graph { steps }`, topologically ordered, last
  step is the root. Iterative, safe for deep graphs.
- `map.rs`: graph + equivalences into egglog (table `symbol/arity`, columns
  attrs + inputs), saturate, extract. Symbols without a kernel are
  `unextractable`. Cost = number of symbols (BigInt, tree cost: shared
  subterms are counted every time they are used). A rule direction is only
  emitted when its right side's variables all occur on the left and the left
  is not a bare variable.
- `run.rs`: `run` calls kernels step by step (first dialect that has one);
  `get` = graph + map + run + downcast.

Dialect crates (workspace members, lib names `np` and `md`):

- `np/`: `Array(Node)`, `Value {shape, data: Vec<f64>}`, add/sub/mul/div/neg,
  `constant`, `random::uniform` (key-passing, splitmix64). One kernel file per
  symbol. Scalars (empty shape) broadcast; otherwise shapes are not checked.
- `md/`: `euler_step`, `euler_10_steps`, each with an equivalence to np and a
  fused kernel.

Tests: `tests/dialect.rs` (a local `toy` dialect), `tests/np.rs`,
`tests/lj.rs`. Examples: `examples/md.rs` (100 np steps map to 10
`md.euler_10_steps`), `examples/argon.rs` (see `argon.md`, which also lists
what was added to the core, np and md after this note was written).

## Things noticed, not yet acted on

- README was rewritten on 2026-11-13 to match the code and `not-a-compiler.md`.
  The project is Rust only: no Python tooling.
- np kernels `expect`/index on malformed attrs or mismatched shapes (panic),
  while nary itself promises not to panic. The kernel signature returns `V`,
  so a kernel has no way to report an error.
- Extraction cost is "fewest symbols". That is intended for now: see
  `not-a-compiler.md`. Saturation has no limit on iterations.
