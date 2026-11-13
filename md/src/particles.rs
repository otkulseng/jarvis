//! Particles that Rust has: where to start them, and what their velocities
//! say. Positions and velocities are arrays of shape (particles, 3), and the
//! particles have the mass one.

use np::Value;

/// Particles on a face-centred cubic lattice that fills a box of this side.
/// The lattice has four places in each of its cells. When that is more
/// places than there are particles, the empty ones are spread evenly among
/// them.
pub fn fcc_lattice(particles: usize, side: f64) -> Value {
    let mut cells = 1;
    while 4 * cells * cells * cells < particles {
        cells += 1;
    }
    let places = 4 * cells * cells * cells;
    let cell = side / cells as f64;
    let within = [[0.0, 0.0, 0.0], [0.5, 0.5, 0.0], [0.5, 0.0, 0.5], [0.0, 0.5, 0.5]];
    let mut data = Vec::with_capacity(3 * particles);
    for particle in 0..particles {
        let place = particle * places / particles;
        let (which, corner) = (place / 4, within[place % 4]);
        let at = [which / (cells * cells), which / cells % cells, which % cells];
        for k in 0..3 {
            data.push((at[k] as f64 + corner[k]) * cell);
        }
    }
    Value {
        shape: vec![particles, 3],
        data,
    }
}

/// The energy of the motion of all the particles.
pub fn kinetic_energy(vel: &Value) -> f64 {
    0.5 * vel.data.iter().map(|v| v * v).sum::<f64>()
}

/// The temperature the velocities amount to, in units where Boltzmann's
/// constant is one. The motion of the particles as a whole does not count.
pub fn temperature(vel: &Value) -> f64 {
    let free = (vel.data.len() as f64 - 3.0).max(1.0);
    2.0 * kinetic_energy(vel) / free
}

/// Take from every particle the velocity that all of them have in common,
/// so that together they go nowhere.
pub fn remove_drift(vel: &mut Value) {
    let particles = (vel.data.len() / 3).max(1) as f64;
    for k in 0..3 {
        let mean = vel.data.iter().skip(k).step_by(3).sum::<f64>() / particles;
        vel.data.iter_mut().skip(k).step_by(3).for_each(|v| *v -= mean);
    }
}

/// Make every velocity larger or smaller by the same factor, so that they
/// amount to this temperature.
pub fn set_temperature(vel: &mut Value, wanted: f64) {
    let now = temperature(vel);
    if now > 0.0 {
        let scale = (wanted / now).sqrt();
        vel.data.iter_mut().for_each(|v| *v *= scale);
    }
}
