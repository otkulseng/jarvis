//! Liquid argon, after A. Rahman, "Correlations in the Motion of Atoms in
//! Liquid Argon", Phys. Rev. 136, A405 (1964): 864 Lennard-Jones atoms in a
//! box that repeats, at 94.4 K and 1.374 g/cm^3.
//!
//! The atoms are started on a lattice, brought to the temperature, and then
//! left alone. Printed at the end: how well the energy was kept, the
//! temperature and potential energy, and the radial distribution g(r).
//!
//! ```sh
//! cargo run --release --example argon             # 864 atoms, as Rahman had
//! cargo run --release --example argon -- 20000    # any number of atoms
//! cargo run --release --features rayon --example argon -- 20000    # on every core
//! ```
//!
//! The density is the same whatever the number of atoms: more atoms are in
//! a larger box.

use std::time::Instant;

/// How many atoms there are, unless the program is given another number.
const NUM_ATOMS: usize = 864;

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
    let atoms: usize = match std::env::args().nth(1) {
        Some(atoms) => atoms.parse().map_err(|_| "the argument is a number of atoms")?,
        None => NUM_ATOMS,
    };
    let dialects = [np::dialect(), md::dialect()];

    let per_volume = DENSITY / MASS * 0.602214 * SIGMA.powi(3);
    let side = (atoms as f64 / per_volume).cbrt();
    let wanted = TEMPERATURE / EPSILON;
    println!(
        "{atoms} atoms, density {per_volume:.4}, temperature {wanted:.4}, box {side:.3} \
         ({:.2} A)",
        side * SIGMA
    );
    if side < 2.0 * CUTOFF {
        let fewest = (per_volume * (2.0 * CUTOFF).powi(3)).ceil();
        return Err(format!(
            "the box must be two cutoffs ({}) a side, which takes {fewest} atoms",
            2.0 * CUTOFF
        ));
    }

    // The same nodes in every graph below.
    let (box_side, cutoff, dt) = (np::constant(side), np::constant(CUTOFF), np::constant(DT));
    let forces = |pos| md::lj_forces(pos, box_side, cutoff);

    // Atoms on a lattice, with random velocities at the temperature.
    let mut pos = md::fcc_lattice(atoms, side);
    let (random, _) = np::random::uniform(np::random::from_seed(1964), vec![atoms, 3]);
    let mut vel = np::get(random, &dialects)?;
    md::remove_drift(&mut vel);
    md::set_temperature(&mut vel, wanted);
    let mut force = np::get(forces(np::array(pos.clone())), &dialects)?;

    // What is measured while the atoms are left alone.
    let mut g = md::RadialDistribution::new(side, 5.0, 0.05);
    let (mut temperatures, mut potentials, mut energies) = (vec![], vec![], vec![]);

    let started = Instant::now();
    println!("    step  temperature  potential/atom  energy/atom");
    for stretch in 0..(SETTLING + MEASURED) / STRIDE {
        // A stretch of steps, as one graph. Nothing is computed here.
        let (mut x, mut v, mut f) = (np::array(pos), np::array(vel), np::array(force));
        for _ in 0..STRIDE {
            (x, v, f) = md::verlet_step(x, v, f, forces, dt);
        }
        let potential = md::lj_energy(x, box_side, cutoff);

        // Here it is computed, and Rust has the atoms again.
        let mut got = np::get_all(&[x, v, f, potential], &dialects)?.into_iter();
        let mut next = || got.next().ok_or("fewer arrays were computed than asked for");
        (pos, vel, force) = (next()?, next()?, next()?);
        let potential = next()?.data[0];

        let step = (stretch + 1) * STRIDE;
        let temperature = md::temperature(&vel);
        let energy = potential + md::kinetic_energy(&vel);
        if step % 500 == 0 {
            println!(
                "{step:8}  {temperature:11.4}  {:14.4}  {:11.5}",
                potential / atoms as f64,
                energy / atoms as f64
            );
        }
        if step <= SETTLING {
            // Bring the atoms to the temperature that is wanted.
            md::set_temperature(&mut vel, wanted);
            if step == SETTLING {
                println!("          (left alone from here)");
            }
        } else {
            g.count(&pos);
            temperatures.push(temperature);
            potentials.push(potential);
            energies.push(energy);
        }
    }
    let seconds = started.elapsed().as_secs_f64();
    println!(
        "{:.2} ms a step\n",
        1000.0 * seconds / (SETTLING + MEASURED) as f64
    );

    let mean = |all: &[f64]| all.iter().sum::<f64>() / all.len() as f64;
    let temperature = mean(&temperatures);
    println!(
        "temperature         {temperature:.4}   ({:.1} K)",
        temperature * EPSILON
    );
    println!(
        "potential per atom  {:.4}  (pairs beyond {CUTOFF} left out, and zero at {CUTOFF})",
        mean(&potentials) / atoms as f64
    );
    let least = energies.iter().copied().fold(f64::INFINITY, f64::min);
    let most = energies.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    println!(
        "energy per atom     {:.5}, kept within {:.1e} of itself",
        energies[0] / atoms as f64,
        (most - least) / energies[0].abs()
    );

    let g = g.values();
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
    Ok(())
}
