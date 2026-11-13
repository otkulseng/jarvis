# Building on the macmini kernel (2026-11-13)

- `pixi run test` does not build here: the system toolchain is rustc/cargo
  1.93.0, and cubecl 0.11.3 (pulled in by `mlip`, a dev-dependency of nary)
  needs rustc 1.95. pixi.toml declares no dependencies, so pixi uses whatever
  cargo is on PATH.
- No PJRT plugin at `.pjrt/libpjrt_cpu.so`, so the StableHLO path cannot run.
- README refers to `DESIGN.md`, which is not in the repository.
- Allowed commands are only `cargo` and `pixi` (.jarvis/claude.json); compound
  shell commands and `rustup` are refused.
- Open question put to the human: pin Rust in pixi.toml (`rust >= 1.95` from
  conda-forge) or update the system toolchain.
