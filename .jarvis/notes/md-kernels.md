# md's kernels, rayon, and what moved out of the argon example (2026-11-13)

- `md/src/kernels/near.rs`: `near_each` computes something for each particle
  from the particles near it (cell list; all particles when the box has
  fewer than three cells a side). Each pair is therefore visited twice, once
  from each end. That is deliberate: every particle is independent, so the
  loop over particles is `into_par_iter()` with the feature `rayon` and a
  plain iterator without, and the two give bit-identical results. Do not go
  back to visiting each pair once (Newton's third law) without the human
  agreeing: it is ~1.45x faster on one core, but cannot be run in parallel
  this simply and would make results depend on the feature.
- Features: `nary-md/rayon`; the root crate has `rayon = ["nary-md/rayon"]`
  so that `cargo test --workspace --features rayon` and
  `cargo run --release --features rayon --example argon` work. Run the tests
  both ways after touching a kernel.
- Out of `examples/argon.rs` into md, at the human's request: `verlet_step`
  (np graph), `fcc_lattice`, `kinetic_energy`, `temperature`, `remove_drift`,
  `set_temperature` (plain Rust on `np::Value`), `RadialDistribution`
  (counts from at most 1000 particles). Reduced units, mass one.
- Timings (release, whole example, ms a step): 3000 atoms 5.62 serial, 1.52
  rayon; 20000 atoms 5.32 rayon (32 s for 6000 steps). Before the per-particle
  rewrite: 1000/2000/3000 atoms 1.81/2.95/3.87 serial.
- An early 20000-atom serial run took >100 ms a step, five times what the
  scaling predicts; never explained. The human does not want 20000 serial
  run again ("excruciatingly slow"). Do not start runs of minutes unasked;
  use rayon or a smaller system, and say beforehand how long a run takes.
