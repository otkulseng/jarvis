//! The smallest whole example: a program written in np, and a dialect made
//! here whose kernels it is mapped onto without naming them.
//!
//! The program moves particles that are pulled towards the origin, a
//! hundred steps of Euler's method. The dialect `toy` has a symbol for one
//! such step and a symbol for ten of them. Each has a meaning in np and a
//! kernel. Printed: the graph after mapping, which is ten of the larger
//! kernel, and the positions.
//!
//! ```sh
//! cargo run --example euler
//! ```

use np::{Array, Value};

/// A dialect of two symbols.
mod toy {
    use super::{step_in_np, Array, Value};

    pub fn euler_step(pos: Array, mass: Array, dt: Array) -> Array {
        nary::symbol("toy.euler_step", [pos, mass, dt])
    }

    pub fn euler_10_steps(pos: Array, mass: Array, dt: Array) -> Array {
        nary::symbol("toy.euler_10_steps", [pos, mass, dt])
    }

    pub fn dialect() -> nary::Dialect {
        nary::Dialect::new("toy")
            .equal(|[pos, mass, dt]| (euler_step(pos, mass, dt), step_in_np(pos, mass, dt)))
            .kernel("toy.euler_step", |x: &[&Value]| steps(1, x))
            .equal(|[pos, mass, dt]| {
                let mut after = pos;
                for _ in 0..10 {
                    after = step_in_np(after, mass, dt);
                }
                (euler_10_steps(pos, mass, dt), after)
            })
            .kernel("toy.euler_10_steps", |x: &[&Value]| steps(10, x))
    }

    /// Some steps in one pass over the particles, with nothing in between.
    fn steps(many: usize, x: &[&Value]) -> Value {
        let (pos, mass, dt) = (x[0], x[1], x[2].data[0]);
        let mut next = pos.clone();
        for (p, m) in next.data.iter_mut().zip(&mass.data) {
            for _ in 0..many {
                *p += dt * (-*p / m);
            }
        }
        next
    }
}

/// One Euler step, in np.
fn step_in_np(pos: Array, mass: Array, dt: Array) -> Array {
    // The force pulls towards the origin.
    let force = np::neg(pos);
    let acc = np::div(force, mass);
    np::add(pos, np::mul(dt, acc))
}

fn main() -> Result<(), String> {
    let key = np::random::from_seed(18);
    let (mut pos, key) = np::random::uniform(key, vec![10, 3]);
    let (mass, _) = np::random::uniform(key, vec![10, 3]);
    let dt = np::constant(0.001);

    // Nothing is computed here: this is the graph of a hundred steps.
    for _ in 0..100 {
        pos = step_in_np(pos, mass, dt);
    }

    let dialects = [np::dialect(), toy::dialect()];
    println!("{}", nary::map(&nary::graph(pos), &dialects)?);
    println!("{:?}", np::get(pos, &dialects)?);
    Ok(())
}
