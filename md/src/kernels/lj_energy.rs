use super::near_each;
use np::Value;

/// The potential energy, from the particles that are near each other only.
pub fn lj_energy(x: &[&Value]) -> Value {
    let (pos, side, cutoff) = (x[0], x[1].data[0], x[2].data[0]);
    let c6 = 1.0 / cutoff.powi(6);
    let at_cutoff = 4.0 * (c6 * c6 - c6);
    let of_each = near_each(pos, side, cutoff, 0.0, |energy, _, r2| {
        let inv6 = 1.0 / (r2 * r2 * r2);
        *energy += 4.0 * (inv6 * inv6 - inv6) - at_cutoff;
    });
    // Each pair was counted from both of its particles.
    Value {
        shape: vec![],
        data: vec![0.5 * of_each.iter().sum::<f64>()],
    }
}
