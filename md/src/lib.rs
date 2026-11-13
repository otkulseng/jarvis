//! md: molecular dynamics, on top of nary.
//!
//! This file is all of md that a program uses: its operations, and the
//! dialect. The code that computes them is in `kernels/`.

mod kernels;

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

/// md: what each of its symbols equals in np, and the kernel that computes it.
pub fn dialect() -> Dialect {
    Dialect::new("md")
        .equal(|[pos, mass, dt]| (euler_step(pos, mass, dt), step_in_np(pos, mass, dt)))
        .kernel("md.euler_step", kernels::euler_step)
        .equal(|[pos, mass, dt]| {
            let mut after = pos;
            for _ in 0..10 {
                after = step_in_np(after, mass, dt);
            }
            (euler_10_steps(pos, mass, dt), after)
        })
        .kernel("md.euler_10_steps", kernels::euler_10_steps)
}

/// One Euler step, in np.
fn step_in_np(pos: Array, mass: Array, dt: Array) -> Array {
    // The force pulls towards the origin.
    let force = np::neg(pos);
    let acc = np::div(force, mass);
    np::add(pos, np::mul(dt, acc))
}
