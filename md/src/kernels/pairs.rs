use np::Value;

/// Call `meet(i, j, apart, r2)` once for each pair of particles nearer than
/// the cutoff: `apart` is the vector from the nearest copy of `j` to `i`,
/// and `r2` its length squared.
///
/// The box is cut into cells no smaller than the cutoff, so a particle can
/// only meet those in its own cell and the 26 around it. That is far fewer
/// than all of them once the box is large. A box of fewer than three cells
/// a side has every pair looked at.
pub fn pairs(pos: &Value, side: f64, cutoff: f64, mut meet: impl FnMut(usize, usize, [f64; 3], f64)) {
    let particles = pos.data.len() / 3;
    let at = |i: usize| [pos.data[3 * i], pos.data[3 * i + 1], pos.data[3 * i + 2]];
    let mut look = |i: usize, j: usize| {
        let (a, b) = (at(i), at(j));
        let mut apart = [0.0; 3];
        let mut r2 = 0.0;
        for k in 0..3 {
            let d = a[k] - b[k];
            apart[k] = d - side * (d / side).round();
            r2 += apart[k] * apart[k];
        }
        if r2 < cutoff * cutoff && r2 > 0.0 {
            meet(i, j, apart, r2);
        }
    };

    let cells = (side / cutoff).floor() as usize;
    if cells < 3 {
        for i in 0..particles {
            for j in i + 1..particles {
                look(i, j);
            }
        }
        return;
    }

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
    // The cell before, the cell itself, the cell after, around the box.
    let around = |c: usize| [(c + cells - 1) % cells, c, (c + 1) % cells];
    for x in 0..cells {
        for y in 0..cells {
            for z in 0..cells {
                let here = &within[(x * cells + y) * cells + z];
                for nx in around(x) {
                    for ny in around(y) {
                        for nz in around(z) {
                            let there = &within[(nx * cells + ny) * cells + nz];
                            for &i in here {
                                for &j in there {
                                    // Each pair is come to from both of its
                                    // particles, and looked at from one.
                                    if i < j {
                                        look(i, j);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
