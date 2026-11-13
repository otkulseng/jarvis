use np::Value;

/// For each particle, what `add` gathers from the particles nearer to it
/// than the cutoff. `add` is given what was gathered so far, the vector
/// from the nearest copy of the other particle to this one, and its length
/// squared.
///
/// The box is cut into cells no smaller than the cutoff, so a particle can
/// only be near those in its own cell and the 26 around it. That is far
/// fewer than all of them once the box is large. A box of fewer than three
/// cells a side has every particle looked at.
///
/// Each particle is worked out on its own, from the positions alone, so
/// they can be worked out at the same time: with the feature `rayon` they
/// are, on every core. The result is the same to the last digit.
pub fn near_each<T: Clone + Send + Sync>(
    pos: &Value,
    side: f64,
    cutoff: f64,
    nothing: T,
    add: impl Fn(&mut T, [f64; 3], f64) + Sync,
) -> Vec<T> {
    let particles = pos.data.len() / 3;
    let at = |i: usize| [pos.data[3 * i], pos.data[3 * i + 1], pos.data[3 * i + 2]];

    let cells = match (side / cutoff).floor() as usize {
        cells if cells < 3 => 1,
        cells => cells,
    };
    // Which cell each particle is in, along each direction.
    let cell_of = |i: usize| {
        at(i).map(|x| {
            let inside = x - side * (x / side).floor();
            ((inside / side * cells as f64) as usize).min(cells - 1)
        })
    };
    let mut within: Vec<Vec<usize>> = vec![vec![]; cells * cells * cells];
    for i in 0..particles {
        let [x, y, z] = cell_of(i);
        within[(x * cells + y) * cells + z].push(i);
    }
    // The cell before, the cell itself and the cell after, around the box.
    let around: &[usize] = if cells == 1 { &[0] } else { &[cells - 1, 0, 1] };

    let one = |i: usize| {
        let mut gathered = nothing.clone();
        let here = at(i);
        let [x, y, z] = cell_of(i);
        for dx in around {
            for dy in around {
                for dz in around {
                    let (x, y, z) = ((x + dx) % cells, (y + dy) % cells, (z + dz) % cells);
                    for &j in &within[(x * cells + y) * cells + z] {
                        let there = at(j);
                        let mut apart = [0.0; 3];
                        let mut r2 = 0.0;
                        for k in 0..3 {
                            let d = here[k] - there[k];
                            apart[k] = d - side * (d / side).round();
                            r2 += apart[k] * apart[k];
                        }
                        if r2 < cutoff * cutoff && r2 > 0.0 {
                            add(&mut gathered, apart, r2);
                        }
                    }
                }
            }
        }
        gathered
    };

    #[cfg(feature = "rayon")]
    {
        use rayon::prelude::*;
        (0..particles).into_par_iter().map(one).collect()
    }
    #[cfg(not(feature = "rayon"))]
    {
        (0..particles).map(one).collect()
    }
}
