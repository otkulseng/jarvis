# Building and checking (macmini, 2026-11-13)

- Rust only. Check with `cargo test --workspace` and
  `cargo run -q --example md`. Both pass with the system Rust 1.93.0; the
  only dependencies are egglog 3 and num.
- The only extra allowed command is `cargo` (.jarvis/claude.json); compound
  shell commands with pipes or `;` are refused, so run one command per call.
- Landing: `git push . HEAD:nary` is refused while `nary` is checked out in
  the human's working copy. Tell the human to `git merge nary@macmini` there.
