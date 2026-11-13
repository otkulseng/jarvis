use np::Value;

/// The whole step in one pass over the particles.
pub fn euler_step(x: &[&Value]) -> Value {
    let (pos, mass, dt) = (x[0], x[1], x[2].data[0]);
    let mut next = pos.clone();
    for (p, m) in next.data.iter_mut().zip(&mass.data) {
        *p += dt * (-*p / m);
    }
    next
}
