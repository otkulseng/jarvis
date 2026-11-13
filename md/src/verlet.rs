//! Stepping particles forward in time.

use np::Array;

/// One step of the velocity Verlet method, written in np: the positions,
/// velocities and forces after the time `dt`. `forces` gives the forces on
/// particles at some positions; the particles have the mass one.
///
/// The forces that come out are those at the new positions, to go into the
/// next step, so each step asks for the forces once.
pub fn verlet_step(
    pos: Array,
    vel: Array,
    force: Array,
    forces: impl Fn(Array) -> Array,
    dt: Array,
) -> (Array, Array, Array) {
    let half_dt = np::mul(np::constant(0.5), dt);
    let halfway = np::add(vel, np::mul(half_dt, force));
    let pos = np::add(pos, np::mul(dt, halfway));
    let force = forces(pos);
    let vel = np::add(halfway, np::mul(half_dt, force));
    (pos, vel, force)
}
