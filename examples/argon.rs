//! Liquid argon, after A. Rahman, "Correlations in the Motion of Atoms in
//! Liquid Argon", Phys. Rev. 136, A405 (1964): 864 Lennard-Jones atoms in a
//! box that repeats, at 94.4 K and 1.374 g/cm^3.
//!
//! The atoms are started on a lattice, brought to the temperature, and then
//! left alone. Printed at the end: how well the energy was kept, the
//! temperature and potential energy, and the radial distribution g(r).
//!
//! ```sh
//! cargo run --release --example argon         # 864 atoms
//! cargo run --release --example argon -- 8    # 2048 atoms
//! ```
//!
//! The argument is the number of lattice cells a side, of four atoms each.

use np::{Array, Value};
use std::time::Instant;

// Argon as Rahman had it.
const SIGMA: f64 = 3.4; // the length unit, in Angstrom
const EPSILON: f64 = 120.0; // the energy unit, in Kelvin
const MASS: f64 = 39.948; // g/mol
const DENSITY: f64 = 1.374; // g/cm^3
const TEMPERATURE: f64 = 94.4; // K

// In units of sigma, epsilon and the mass of an atom.
const CUTOFF: f64 = 2.5;
const DT: f64 = 0.005; // 0.0108 ps
/// Steps between two times that Rust looks at the atoms.
const STRIDE: usize = 100;
const SETTLING: usize = 2000;
const MEASURED: usize = 4000;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cells: usize = match args.first() {
        Some(cells) => cells.parse().map_err(|_| "the argument is a number of cells")?,
        None => 6,
    };
    let dialects = [np::dialect(), md::dialect()];

    let atoms = 4 * cells.pow(3);
    let per_volume = DENSITY / MASS * 0.602214 * SIGMA.powi(3);
    let side = (atoms as f64 / per_volume).cbrt();
    let wanted = TEMPERATURE / EPSILON;
    println!(
        "{atoms} atoms, density {per_volume:.4}, temperature {wanted:.4}, box {side:.3} \
         ({:.2} A)",
        side * SIGMA
    );
    if side < 2.0 * CUTOFF {
        return Err(format!("the box must be two cutoffs ({}) a side", 2.0 * CUTOFF));
    }

    // The same nodes in every graph below.
    let (box_side, cutoff) = (np::constant(side), np::constant(CUTOFF));
    let (dt, half_dt) = (np::constant(DT), np::constant(DT / 2.0));

    let mut pos = lattice(cells, side);
    let mut vel = velocities(atoms, wanted, &dialects)?;
    let mut force = np::get(md::lj_forces(np::array(pos.clone()), box_side, cutoff), &dialects)?;

    let mut measured = Measured::new(atoms, side);
    let started = Instant::now();
    println!("    step  temperature  potential/atom  energy/atom");
    for stretch in 0..(SETTLING + MEASURED) / STRIDE {
        // A stretch of steps, as one graph. Nothing is computed here.
        let (mut x, mut v, mut f) = (np::array(pos), np::array(vel), np::array(force));
        for _ in 0..STRIDE {
            (x, v, f) = verlet_step(x, v, f, |x| md::lj_forces(x, box_side, cutoff), dt, half_dt);
        }
        let potential = md::lj_energy(x, box_side, cutoff);

        // Here it is computed, and Rust has the atoms again.
        let mut got = np::get_all(&[x, v, f, potential], &dialects)?.into_iter();
        let mut next = || got.next().ok_or("fewer arrays were computed than asked for");
        (pos, vel, force) = (next()?, next()?, next()?);
        let potential = next()?.data[0];

        let step = (stretch + 1) * STRIDE;
        let kinetic = 0.5 * vel.data.iter().map(|v| v * v).sum::<f64>();
        let temperature = 2.0 * kinetic / (3 * atoms - 3) as f64;
        if step % 500 == 0 {
            println!(
                "{step:8}  {temperature:11.4}  {:14.4}  {:11.5}",
                potential / atoms as f64,
                (potential + kinetic) / atoms as f64
            );
        }
        if step <= SETTLING {
            // Bring the atoms to the temperature that is wanted.
            let scale = (wanted / temperature).sqrt();
            vel.data.iter_mut().for_each(|v| *v *= scale);
            if step == SETTLING {
                println!("          (left alone from here)");
            }
        } else {
            measured.take(&pos, temperature, potential, kinetic);
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "{:.2} ms a step\n",
        1000.0 * seconds / (SETTLING + MEASURED) as f64
    );
    measured.print();
    Ok(())
}

/// One step of the velocity Verlet method, written in np: the positions,
/// velocities and forces after the time `dt`. The atoms have the mass one.
fn verlet_step(
    pos: Array,
    vel: Array,
    force: Array,
    forces: impl Fn(Array) -> Array,
    dt: Array,
    half_dt: Array,
) -> (Array, Array, Array) {
    let halfway = np::add(vel, np::mul(half_dt, force));
    let pos = np::add(pos, np::mul(dt, halfway));
    let force = forces(pos);
    let vel = np::add(halfway, np::mul(half_dt, force));
    (pos, vel, force)
}

/// Atoms on a face-centred cubic lattice that fills the box.
fn lattice(cells: usize, side: f64) -> Value {
    let cell = side / cells as f64;
    let within = [[0.0, 0.0, 0.0], [0.5, 0.5, 0.0], [0.5, 0.0, 0.5], [0.0, 0.5, 0.5]];
    let mut data = Vec::new();
    for x in 0..cells {
        for y in 0..cells {
            for z in 0..cells {
                for atom in within {
                    data.push((x as f64 + atom[0]) * cell);
                    data.push((y as f64 + atom[1]) * cell);
                    data.push((z as f64 + atom[2]) * cell);
                }
            }
        }
    }
    Value {
        shape: vec![data.len() / 3, 3],
        data,
    }
}

/// Random velocities that carry the atoms nowhere as a whole, at the
/// temperature that is wanted.
fn velocities(atoms: usize, wanted: f64, dialects: &[nary::Dialect]) -> Result<Value, String> {
    let (random, _) = np::random::uniform(np::random::from_seed(1964), vec![atoms, 3]);
    let mut vel = np::get(random, dialects)?;
    for k in 0..3 {
        let mean = vel.data.iter().skip(k).step_by(3).sum::<f64>() / atoms as f64;
        vel.data.iter_mut().skip(k).step_by(3).for_each(|v| *v -= mean);
    }
    let temperature = vel.data.iter().map(|v| v * v).sum::<f64>() / (3 * atoms - 3) as f64;
    let scale = (wanted / temperature).sqrt();
    vel.data.iter_mut().for_each(|v| *v *= scale);
    Ok(vel)
}

/// What is measured while the atoms are left alone.
struct Measured {
    atoms: usize,
    side: f64,
    times: usize,
    temperature: f64,
    potential: f64,
    /// The total energy: the first, and the least and the most since.
    energy: Option<(f64, f64, f64)>,
    /// How many pairs of atoms were at each distance, in bins of `BIN`.
    distances: Vec<usize>,
}

const BIN: f64 = 0.05;

impl Measured {
    fn new(atoms: usize, side: f64) -> Measured {
        Measured {
            atoms,
            side,
            times: 0,
            temperature: 0.0,
            potential: 0.0,
            energy: None,
            distances: vec![0; (side / 2.0 / BIN) as usize],
        }
    }

    fn take(&mut self, pos: &Value, temperature: f64, potential: f64, kinetic: f64) {
        self.times += 1;
        self.temperature += temperature;
        self.potential += potential;
        let energy = potential + kinetic;
        let (first, least, most) = self.energy.unwrap_or((energy, energy, energy));
        self.energy = Some((first, least.min(energy), most.max(energy)));
        for i in 0..self.atoms {
            for j in i + 1..self.atoms {
                let mut r2 = 0.0;
                for k in 0..3 {
                    let d = pos.data[3 * i + k] - pos.data[3 * j + k];
                    let d = d - self.side * (d / self.side).round();
                    r2 += d * d;
                }
                if let Some(bin) = self.distances.get_mut((r2.sqrt() / BIN) as usize) {
                    *bin += 1;
                }
            }
        }
    }

    fn print(&self) {
        let (atoms, times) = (self.atoms as f64, self.times as f64);
        let temperature = self.temperature / times;
        println!(
            "temperature         {temperature:.4}   ({:.1} K)",
            temperature * EPSILON
        );
        println!(
            "potential per atom  {:.4}  (pairs beyond {CUTOFF} left out, and zero at {CUTOFF})",
            self.potential / times / atoms
        );
        if let Some((first, least, most)) = self.energy {
            println!(
                "energy per atom     {:.5}, kept within {:.1e} of itself",
                first / atoms,
                (most - least) / first.abs()
            );
        }

        // How many atoms are at each distance from an atom, against how
        // many there would be if they were spread evenly.
        let per_volume = atoms / self.side.powi(3);
        let g: Vec<(f64, f64)> = self
            .distances
            .iter()
            .enumerate()
            .map(|(bin, pairs)| {
                let (near, far) = (bin as f64 * BIN, (bin + 1) as f64 * BIN);
                let shell = 4.0 / 3.0 * std::f64::consts::PI * (far.powi(3) - near.powi(3));
                let even = 0.5 * atoms * per_volume * shell * times;
                ((near + far) / 2.0, *pairs as f64 / even)
            })
            .collect();
        if let Some((r, most)) = g.iter().copied().max_by(|a, b| a.1.total_cmp(&b.1)) {
            println!(
                "g(r) is highest at  r = {r:.3} ({:.2} A), where it is {most:.2}\n",
                r * SIGMA
            );
        }
        println!("    r    r (A)    g(r)");
        for (r, g) in g {
            println!("{r:5.3}  {:7.3}  {g:6.3}  {}", r * SIGMA, "#".repeat((g * 12.0) as usize));
        }
    }
}
