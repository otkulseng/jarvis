//! md: molecular dynamics, on top of nary.
//!
//! This file is all of md that a program uses: its operations, and the
//! dialect. What the operations mean is in `lj.rs`, and the code that
//! computes them in `kernels/`.

mod kernels;
mod lj;

pub use lj::{lj_energy, lj_forces};

use nary::Dialect;

/// What each symbol of md equals in np, and no kernels: with this and np, a
/// program in md is computed by np's kernels alone. It is what the kernels
/// of md are checked against.
pub fn meaning() -> Dialect {
    Dialect::new("md")
        .equal(|[pos, side, cutoff]| {
            (
                lj_forces(pos, side, cutoff),
                lj::forces_in_np(pos, side, cutoff),
            )
        })
        .equal(|[pos, side, cutoff]| {
            (
                lj_energy(pos, side, cutoff),
                lj::energy_in_np(pos, side, cutoff),
            )
        })
}

/// md: what each of its symbols equals in np, and the kernel that computes it.
pub fn dialect() -> Dialect {
    meaning()
        .kernel("md.lj_forces", kernels::lj_forces)
        .kernel("md.lj_energy", kernels::lj_energy)
}
