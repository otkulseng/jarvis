//! md: molecular dynamics, on top of nary.
//!
//! This file is all of md that a program uses: its operations, and the
//! dialect. What the operations equal in np is in `lj.rs`, and the code
//! that computes them in `kernels/`.

mod kernels;
mod lj;

pub use lj::{lj_energy, lj_forces};

use nary::Dialect;

/// md: what each of its symbols equals in np, and the kernel that computes it.
pub fn dialect() -> Dialect {
    Dialect::new("md")
        .equal(|[pos, side, cutoff]| {
            (
                lj_forces(pos, side, cutoff),
                lj::forces_in_np(pos, side, cutoff),
            )
        })
        .kernel("md.lj_forces", kernels::lj_forces)
        .equal(|[pos, side, cutoff]| {
            (
                lj_energy(pos, side, cutoff),
                lj::energy_in_np(pos, side, cutoff),
            )
        })
        .kernel("md.lj_energy", kernels::lj_energy)
}
