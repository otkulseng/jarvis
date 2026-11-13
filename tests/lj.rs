//! Lennard-Jones particles: the kernels of md against what they mean in np.

use np::{Array, Value};

/// Particles near the places of a cubic lattice of `cells` a side, at the
/// density 0.8, and the side of the box they are in.
fn particles(cells: usize) -> (Array, f64) {
    let side = (cells.pow(3) as f64 / 0.8).cbrt();
    let key = np::random::from_seed(7);
    let (noise, _) = np::random::uniform(key, vec![cells.pow(3), 3]);
    let noise = np::get(noise, &[np::dialect()]).unwrap();
    let mut data = Vec::new();
    for i in 0..cells.pow(3) {
        let place = [i / (cells * cells), i / cells % cells, i % cells];
        for k in 0..3 {
            let moved = 0.3 * (noise.data[3 * i + k] - 0.5);
            // Some of them a box or two away: only the nearest copy counts.
            let boxes = (i % 3) as f64 - 1.0;
            data.push((place[k] as f64 + moved) * side / cells as f64 + boxes * side);
        }
    }
    let shape = vec![cells.pow(3), 3];
    (np::array(Value { shape, data }), side)
}

fn close(a: &Value, b: &Value) {
    assert_eq!(a.shape, b.shape);
    let largest = b.data.iter().fold(1.0f64, |m, x| m.max(x.abs()));
    for (x, y) in a.data.iter().zip(&b.data) {
        assert!((x - y).abs() < 1e-10 * largest, "{x} and {y}");
    }
}

/// Forces and energy by md's kernels, and by np's alone.
fn both_ways(cells: usize) -> (Vec<Value>, Vec<Value>) {
    let (pos, side) = particles(cells);
    let (side, cutoff) = (np::constant(side), np::constant(2.5));
    let asked = [
        md::lj_forces(pos, side, cutoff),
        md::lj_energy(pos, side, cutoff),
    ];

    let with_kernels = [np::dialect(), md::dialect()];
    let plan = nary::map(&nary::graph_all(asked), &with_kernels).unwrap();
    assert_eq!(plan.steps.len(), 5, "{plan}");

    let in_np = [np::dialect(), md::meaning()];
    let plan = nary::map(&nary::graph_all(asked), &in_np).unwrap();
    assert!(!plan.to_string().contains("md."), "{plan}");

    (
        np::get_all(&asked, &with_kernels).unwrap(),
        np::get_all(&asked, &in_np).unwrap(),
    )
}

#[test]
fn the_kernels_agree_with_np_when_every_pair_is_looked_at() {
    // A box of two cutoffs a side: too small to cut into cells.
    let (kernels, np) = both_ways(4);
    close(&kernels[0], &np[0]);
    close(&kernels[1], &np[1]);
}

#[test]
fn the_kernels_agree_with_np_when_the_box_is_cut_into_cells() {
    let (kernels, np) = both_ways(7);
    close(&kernels[0], &np[0]);
    close(&kernels[1], &np[1]);
    // The forces of the pairs cancel.
    for k in 0..3 {
        let total: f64 = kernels[0].data.iter().skip(k).step_by(3).sum();
        assert!(total.abs() < 1e-9, "{total}");
    }
}

#[test]
fn the_force_is_how_the_energy_changes() {
    let (pos, side) = particles(4);
    let dialects = [np::dialect(), md::dialect()];
    let (side, cutoff) = (np::constant(side), np::constant(2.5));
    let at = np::get(pos, &dialects).unwrap();
    let forces = np::get(md::lj_forces(pos, side, cutoff), &dialects).unwrap();
    let energy = |moved: &Value| {
        let moved = np::array(moved.clone());
        np::get(md::lj_energy(moved, side, cutoff), &dialects).unwrap().data[0]
    };
    for which in [0, 50, 191] {
        let (mut ahead, mut behind) = (at.clone(), at.clone());
        ahead.data[which] += 1e-5;
        behind.data[which] -= 1e-5;
        let slope = (energy(&ahead) - energy(&behind)) / 2e-5;
        let force = forces.data[which];
        assert!((force + slope).abs() < 1e-5 * force.abs().max(1.0), "{force} and {slope}");
    }
}

#[test]
fn a_program_written_in_np_alone_is_mapped_onto_the_kernel() {
    // The same mathematics as md's, written here: md is never called.
    let (pos, side) = particles(4);
    let (side, cutoff) = (np::constant(side), np::constant(2.5));
    let apart = np::sub(np::expand_dims(pos, 1), np::expand_dims(pos, 0));
    let apart = np::sub(apart, np::mul(side, np::round(np::div(apart, side))));
    let r2 = np::sum(np::mul(apart, apart), 2);
    let other = np::less(np::constant(0.0), r2);
    let meets = np::mul(other, np::less(r2, np::mul(cutoff, cutoff)));
    let r2 = np::add(r2, np::sub(np::constant(1.0), other));
    let inv2 = np::div(meets, r2);
    let inv6 = np::mul(inv2, np::mul(inv2, inv2));
    let twice = np::mul(np::constant(2.0), np::mul(inv6, inv6));
    let strength = np::mul(np::constant(24.0), np::mul(inv2, np::sub(twice, inv6)));
    let forces = np::sum(np::mul(np::expand_dims(strength, 2), apart), 1);

    let plan = nary::map(&nary::graph(forces), &[np::dialect(), md::dialect()]).unwrap();
    assert!(plan.to_string().contains("md.lj_forces"), "{plan}");
    assert_eq!(plan.steps.len(), 4, "{plan}");
}
