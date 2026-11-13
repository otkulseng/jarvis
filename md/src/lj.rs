//! The Lennard-Jones potential, for particles in a box that repeats in
//! every direction.
//!
//! Lengths are in units of sigma and energies in units of epsilon, so a
//! pair at the distance r has the energy 4 (r^-12 - r^-6). Each particle
//! meets the nearest copy of each other one, and only within the cutoff.
//! The energy of a pair is moved so that it is zero at the cutoff.
//!
//! The force is written out by hand, next to the energy it comes from.

use nary::symbol;
use np::Array;

/// The force on each particle: an array like `pos`, of shape (particles, 3).
/// `side` is the length of the box and `cutoff` the distance beyond which
/// two particles do not meet; both are numbers.
pub fn lj_forces(pos: Array, side: Array, cutoff: Array) -> Array {
    symbol("md.lj_forces", [pos, side, cutoff])
}

/// The potential energy of all the particles: a number.
pub fn lj_energy(pos: Array, side: Array, cutoff: Array) -> Array {
    symbol("md.lj_energy", [pos, side, cutoff])
}

/// Every pair of particles, in np: the vector from the second to the first,
/// of shape (particles, particles, 3), and 1/r^2 for the pairs that meet,
/// zero for the others, of shape (particles, particles).
fn pairs(pos: Array, side: Array, cutoff: Array) -> (Array, Array) {
    let (zero, one) = (np::constant(0.0), np::constant(1.0));
    let apart = np::sub(np::expand_dims(pos, 1), np::expand_dims(pos, 0));
    // To the nearest copy.
    let apart = np::sub(apart, np::mul(side, np::round(np::div(apart, side))));
    let r2 = np::sum(np::mul(apart, apart), 2);
    // A particle does not meet itself.
    let other = np::less(zero, r2);
    let meets = np::mul(other, np::less(r2, np::mul(cutoff, cutoff)));
    // One instead of zero where a particle is paired with itself, so that
    // nothing is divided by zero.
    let r2 = np::add(r2, np::sub(one, other));
    (apart, np::div(meets, r2))
}

/// `lj_forces`, in np.
pub(crate) fn forces_in_np(pos: Array, side: Array, cutoff: Array) -> Array {
    let (apart, inv2) = pairs(pos, side, cutoff);
    let inv6 = np::mul(inv2, np::mul(inv2, inv2));
    // 24 (2 r^-12 - r^-6) / r^2, times the vector between the two.
    let twice = np::mul(np::constant(2.0), np::mul(inv6, inv6));
    let strength = np::mul(np::constant(24.0), np::mul(inv2, np::sub(twice, inv6)));
    np::sum(np::mul(np::expand_dims(strength, 2), apart), 1)
}

/// `lj_energy`, in np.
pub(crate) fn energy_in_np(pos: Array, side: Array, cutoff: Array) -> Array {
    let (_, inv2) = pairs(pos, side, cutoff);
    let inv6 = np::mul(inv2, np::mul(inv2, inv2));
    let four = np::constant(4.0);
    let pair = np::mul(four, np::sub(np::mul(inv6, inv6), inv6));
    // What a pair has at the cutoff, taken from each pair that meets.
    let c2 = np::div(np::constant(1.0), np::mul(cutoff, cutoff));
    let c6 = np::mul(c2, np::mul(c2, c2));
    let at_cutoff = np::mul(four, np::sub(np::mul(c6, c6), c6));
    // 1/r^2 is zero where a pair does not meet, so this is one where it does.
    let meets = np::less(np::constant(0.0), inv2);
    let pair = np::sub(pair, np::mul(meets, at_cutoff));
    // Each pair was counted from both of its particles.
    np::mul(np::constant(0.5), np::sum(np::sum(pair, 1), 0))
}
