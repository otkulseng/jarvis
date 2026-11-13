//! The radial distribution g(r): how many particles are at each distance
//! from a particle, against how many there would be if they were spread
//! evenly.

use np::Value;

/// Distances are counted from this many particles at most, to all the
/// others: from every one of very many, the counting would take longer than
/// the simulation.
const COUNTED_FROM: usize = 1000;

/// g(r), gathered from the positions at several times.
pub struct RadialDistribution {
    side: f64,
    bin: f64,
    /// How many particles were found in each bin of distance.
    found: Vec<usize>,
    /// How many particles the distances were counted from, over all times.
    from: usize,
    /// How many particles there are in the box.
    particles: usize,
}

impl RadialDistribution {
    /// For particles in a box of this side that repeats: distances up to
    /// `farthest`, or half the box if that is less, in bins this wide.
    pub fn new(side: f64, farthest: f64, bin: f64) -> RadialDistribution {
        RadialDistribution {
            side,
            bin,
            found: vec![0; ((side / 2.0).min(farthest) / bin) as usize],
            from: 0,
            particles: 0,
        }
    }

    /// Count the distances between the particles as they are now.
    pub fn count(&mut self, pos: &Value) {
        let particles = pos.data.len() / 3;
        self.particles = particles;
        self.from += particles.min(COUNTED_FROM);
        for i in 0..particles.min(COUNTED_FROM) {
            for j in 0..particles {
                if i == j {
                    continue;
                }
                let mut r2 = 0.0;
                for k in 0..3 {
                    let d = pos.data[3 * i + k] - pos.data[3 * j + k];
                    let d = d - self.side * (d / self.side).round();
                    r2 += d * d;
                }
                if let Some(bin) = self.found.get_mut((r2.sqrt() / self.bin) as usize) {
                    *bin += 1;
                }
            }
        }
    }

    /// The middle of each bin, and g there.
    pub fn values(&self) -> Vec<(f64, f64)> {
        let per_volume = self.particles as f64 / self.side.powi(3);
        self.found
            .iter()
            .enumerate()
            .map(|(bin, found)| {
                let (near, far) = (bin as f64 * self.bin, (bin + 1) as f64 * self.bin);
                let shell = 4.0 / 3.0 * std::f64::consts::PI * (far.powi(3) - near.powi(3));
                let even = self.from as f64 * per_volume * shell;
                ((near + far) / 2.0, *found as f64 / even)
            })
            .collect()
    }
}
