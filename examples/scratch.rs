// Temporary: how the force kernel and a stretch scale with the atoms.
use np::Value;
use std::time::Instant;

fn main() {
    let dialects = [np::dialect(), md::dialect()];
    for atoms in [864usize, 5000, 20000] {
        let side = (atoms as f64 / 0.8141).cbrt();
        let (random, _) = np::random::uniform(np::random::from_seed(1), vec![atoms, 3]);
        let mut pos: Value = np::get(random, &dialects).unwrap();
        pos.data.iter_mut().for_each(|x| *x *= side);
        let (box_side, cutoff) = (np::constant(side), np::constant(2.5));
        let dt = np::constant(1e-12);

        let x = np::array(pos.clone());
        let f = md::lj_forces(x, box_side, cutoff);
        let mapped = nary::map(&nary::graph(f), &dialects).unwrap();
        let t = Instant::now();
        for _ in 0..5 {
            nary::run(&mapped, &dialects).unwrap();
        }
        let one_force = t.elapsed() / 5;

        let (mut x, mut v, mut f) = (x, f, f);
        for _ in 0..20 {
            let halfway = np::add(v, np::mul(dt, f));
            x = np::add(x, np::mul(dt, halfway));
            f = md::lj_forces(x, box_side, cutoff);
            v = np::add(halfway, np::mul(dt, f));
        }
        let t = Instant::now();
        let graph = nary::graph_all([x, v, f]);
        let mapped = nary::map(&graph, &dialects).unwrap();
        let t_map = t.elapsed();
        let t = Instant::now();
        nary::run(&mapped, &dialects).unwrap();
        let t_run = t.elapsed() / 20;
        println!("{atoms:6} atoms: one force {one_force:?}; 20 steps: map {t_map:?}, run {t_run:?} a step, {} steps mapped", mapped.steps.len());
    }
}
