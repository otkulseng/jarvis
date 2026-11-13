use super::pairs;
use np::Value;

/// The potential energy, from the pairs that are near each other only.
pub fn lj_energy(x: &[&Value]) -> Value {
    let (pos, side, cutoff) = (x[0], x[1].data[0], x[2].data[0]);
    let c6 = 1.0 / cutoff.powi(6);
    let at_cutoff = 4.0 * (c6 * c6 - c6);
    let mut energy = 0.0;
    pairs(pos, side, cutoff, |_, _, _, r2| {
        let inv6 = 1.0 / (r2 * r2 * r2);
        energy += 4.0 * (inv6 * inv6 - inv6) - at_cutoff;
    });
    Value {
        shape: vec![],
        data: vec![energy],
    }
}
