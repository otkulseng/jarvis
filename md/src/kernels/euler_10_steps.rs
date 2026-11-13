use np::Value;

/// All ten steps in one pass over the particles, with nothing in between.
pub fn euler_10_steps(x: &[&Value]) -> Value {
    let (pos, mass, dt) = (x[0], x[1], x[2].data[0]);
    let mut next = pos.clone();
    for (p, m) in next.data.iter_mut().zip(&mass.data) {
        for _ in 0..10 {
            *p += dt * (-*p / m);
        }
    }
    next
}
