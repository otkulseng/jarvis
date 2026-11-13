//! md: molecular dynamics, on top of nary.
//!
//! This file is all of md that a program uses.
//!
//! ```text
//!   lj.rs         the symbols of the dialect, and what they equal in np
//!   kernels/      the code that computes them
//!   verlet.rs     a time step, written in np
//!   particles.rs  where particles start, and their temperature
//!   radial.rs     the radial distribution g(r)
//! ```
//!
//! The first three build graphs. The last two are ordinary Rust, for the
//! values a program has between two graphs.
//!
//! With the feature `rayon`, the kernels use every core of the machine.
//! They compute the same numbers either way.

mod kernels;
mod lj;
mod particles;
mod radial;
mod verlet;

pub use lj::{lj_energy, lj_forces};
pub use particles::{fcc_lattice, kinetic_energy, remove_drift, set_temperature, temperature};
pub use radial::RadialDistribution;
pub use verlet::verlet_step;

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
