//! md: molecular dynamics, on top of nary.
//!
//! This file is all of md that a program uses: its operations, and the
//! dialect. The code that computes them is in `kernels/`.

mod kernels;
mod lj;

pub use lj::{lj_energy, lj_forces};

use nary::{symbol, Dialect};
use np::Array;

/// One step of Euler's method, for particles pulled towards the origin:
/// the positions after the time `dt`.
pub fn euler_step(pos: Array, mass: Array, dt: Array) -> Array {
    symbol("md.euler_step", [pos, mass, dt])
}

/// Ten of those steps, one after the other.
pub fn euler_10_steps(pos: Array, mass: Array, dt: Array) -> Array {
    symbol("md.euler_10_steps", [pos, mass, dt])
}

/// What each symbol of md equals in np, and no kernels: with this and np, a
/// program in md is computed by np's kernels alone. It is what the kernels
/// of md are checked against.
pub fn meaning() -> Dialect {
    Dialect::new("md")
        .equal(|[pos, mass, dt]| (euler_step(pos, mass, dt), step_in_np(pos, mass, dt)))
        .equal(|[pos, mass, dt]| {
            let mut after = pos;
            for _ in 0..10 {
                after = step_in_np(after, mass, dt);
            }
            (euler_10_steps(pos, mass, dt), after)
        })
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
        .kernel("md.euler_step", kernels::euler_step)
        .kernel("md.euler_10_steps", kernels::euler_10_steps)
        .kernel("md.lj_forces", kernels::lj_forces)
        .kernel("md.lj_energy", kernels::lj_energy)
}

/// One Euler step, in np.
fn step_in_np(pos: Array, mass: Array, dt: Array) -> Array {
    // The force pulls towards the origin.
    let force = np::neg(pos);
    let acc = np::div(force, mass);
    np::add(pos, np::mul(dt, acc))
}
