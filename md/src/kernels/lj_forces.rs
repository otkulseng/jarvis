use super::pairs;
use np::Value;

/// The force on each particle, from the pairs that are near each other
/// only: each pair is met once and pushes its two particles opposite ways.
pub fn lj_forces(x: &[&Value]) -> Value {
    let (pos, side, cutoff) = (x[0], x[1].data[0], x[2].data[0]);
    let mut forces = vec![0.0; pos.data.len()];
    pairs(pos, side, cutoff, |i, j, apart, r2| {
        let inv2 = 1.0 / r2;
        let inv6 = inv2 * inv2 * inv2;
        let strength = 24.0 * inv2 * (2.0 * inv6 * inv6 - inv6);
        for k in 0..3 {
            forces[3 * i + k] += strength * apart[k];
            forces[3 * j + k] -= strength * apart[k];
        }
    });
    Value {
        shape: pos.shape.clone(),
        data: forces,
    }
}
