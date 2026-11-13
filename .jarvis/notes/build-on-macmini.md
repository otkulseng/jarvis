# Building and checking (macmini, 2026-11-13)

- Check with `cargo test --workspace` and `cargo run -q --example md`. Both
  pass with the system Rust 1.93.0 since the rewrite (cubecl and pjrt are
  gone; the only dependencies are egglog 3 and num).
- pixi.toml has no tasks any more: `pixi run test` fails. Use cargo directly.
- Allowed commands are only `cargo` and `pixi` (.jarvis/claude.json); compound
  shell commands with pipes or `;` are refused, so run one command per call.
