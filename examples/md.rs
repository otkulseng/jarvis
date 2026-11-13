// Small md example

use std::println;

fn main() {
    let key = np::random::from_seed(18);
    let (mut pos, key) = np::random::uniform(key, vec![10, 3]);
    let (mass, _key) = np::random::uniform(key, vec![10, 3]);
    let dt = np::constant(1e-15);
    for _ in 0..100 {
        // Force towards zero
        let force = np::neg(pos);
        let acc = np::div(force, mass);

        // euler
        pos = np::add(pos, np::mul(dt, acc));
    }

    pos = np::add(pos, pos);

    // No computation yet, just create entire graph, map to kernels, and execute
    let dialects = [np::dialect(), md::dialect()];

    println!("{}", nary::map(&nary::graph(pos), &dialects).unwrap());
    if let Ok(val) = np::get(pos, &dialects) {
        println!("final positions {:?}", val)
    } else {
        println!("failed!");
    }
}
