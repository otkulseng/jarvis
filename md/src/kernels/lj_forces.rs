use super::near_each;
use np::Value;

/// The force on each particle, from the particles that are near it only.
pub fn lj_forces(x: &[&Value]) -> Value {
    let (pos, side, cutoff) = (x[0], x[1].data[0], x[2].data[0]);
    let forces = near_each(pos, side, cutoff, [0.0; 3], |force, apart, r2| {
        let inv2 = 1.0 / r2;
        let inv6 = inv2 * inv2 * inv2;
        let strength = 24.0 * inv2 * (2.0 * inv6 * inv6 - inv6);
        for k in 0..3 {
            force[k] += strength * apart[k];
        }
    });
    Value {
        shape: pos.shape.clone(),
        data: forces.into_iter().flatten().collect(),
    }
}
