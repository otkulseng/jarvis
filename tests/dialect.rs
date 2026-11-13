//! Making a dialect: a symbol, what it equals in another dialect, a kernel.

use np::Array;

/// A dialect of one symbol: an Euler step.
mod toy {
    use np::{Array, Value};

    pub fn euler_step(pos: Array, mass: Array, dt: Array) -> Array {
        nary::symbol("toy.euler_step", [pos, mass, dt])
    }

    /// The step is a name for some mathematics in np.
    pub fn meaning() -> nary::Dialect {
        nary::Dialect::new("toy").equal(|[pos, mass, dt]| {
            (
                euler_step(pos, mass, dt),
                np::add(pos, np::mul(dt, np::div(np::neg(pos), mass))),
            )
        })
    }

    /// The same, and one piece of code that computes the whole step.
    pub fn dialect() -> nary::Dialect {
        meaning().kernel("toy.euler_step", |x: &[&Value]| {
            let (pos, mass, dt) = (x[0], x[1], x[2].data[0]);
            let data = pos
                .data
                .iter()
                .zip(&mass.data)
                .map(|(p, m)| p + dt * (-p / m))
                .collect();
            Value {
                shape: pos.shape.clone(),
                data,
            }
        })
    }
}

fn two_arrays() -> (Array, Array) {
    let key = np::random::from_seed(1);
    let (pos, key) = np::random::uniform(key, vec![4, 3]);
    let (mass, _) = np::random::uniform(key, vec![4, 3]);
    (pos, mass)
}

fn step_in_np(pos: Array, mass: Array, dt: Array) -> Array {
    np::add(pos, np::mul(dt, np::div(np::neg(pos), mass)))
}

fn mapped(a: Array, dialects: &[nary::Dialect]) -> String {
    nary::map(&nary::graph(a), dialects).unwrap().to_string()
}

#[test]
fn a_symbol_with_only_a_meaning_is_mapped_onto_what_it_equals() {
    let (pos, mass) = two_arrays();
    let dt = np::constant(0.5);
    let dialects = [np::dialect(), toy::meaning()];
    let step = toy::euler_step(pos, mass, dt);
    assert!(!mapped(step, &dialects).contains("toy.euler_step"));
    assert_eq!(
        np::get(step, &dialects),
        np::get(step_in_np(pos, mass, dt), &[np::dialect()])
    );
}

#[test]
fn a_graph_in_np_is_mapped_onto_the_kernel_of_another_dialect() {
    let (pos, mass) = two_arrays();
    let dt = np::constant(0.5);
    // Two steps, written in np. toy is never called.
    let two = step_in_np(step_in_np(pos, mass, dt), mass, dt);

    let with_np = mapped(two, &[np::dialect()]);
    assert_eq!(with_np.matches("toy.euler_step").count(), 0, "{with_np}");

    // The same graph, given toy as well: the two arrays, the number, and the
    // two steps. None of np's arithmetic is left.
    let dialects = [np::dialect(), toy::dialect()];
    let with_md = mapped(two, &dialects);
    assert_eq!(with_md.matches("toy.euler_step").count(), 2, "{with_md}");
    assert_eq!(with_md.lines().count(), 5, "{with_md}");

    // Both ways of computing it agree.
    assert_eq!(np::get(two, &dialects), np::get(two, &[np::dialect()]));
}

#[test]
fn a_symbol_with_only_a_kernel_is_a_primitive_of_its_own() {
    let (pos, mass) = two_arrays();
    let only_kernel =
        nary::Dialect::new("toy").kernel("toy.euler_step", |x: &[&np::Value]| x[0].clone());
    let step = toy::euler_step(pos, mass, np::constant(0.5));
    let plan = mapped(step, &[np::dialect(), only_kernel]);
    assert!(plan.contains("toy.euler_step"), "{plan}");
}

#[test]
fn a_graph_that_cannot_be_mapped_says_which_symbols_have_no_kernel() {
    let (pos, mass) = two_arrays();
    let step = toy::euler_step(pos, mass, np::constant(0.5));
    let why = np::get(step, &[np::dialect()]).unwrap_err();
    assert!(
        why.contains("The symbols in it without one: `toy.euler_step`"),
        "{why}"
    );
}

#[test]
fn what_is_wrong_with_a_dialect_is_an_error_and_not_a_panic() {
    let (pos, _) = two_arrays();
    // An equivalence with both sides in one dialect.
    let within =
        nary::Dialect::new("np").equal(|[a, b]: [Array; 2]| (np::add(a, b), np::add(b, a)));
    let why = np::get(pos, &[np::dialect(), within]).unwrap_err();
    assert!(why.contains("an equivalence is between dialects"), "{why}");

    // A kernel for another dialect's symbol.
    let not_its_own = nary::Dialect::new("toy").kernel("np.add", |x: &[&np::Value]| x[0].clone());
    let why = np::get(pos, &[np::dialect(), not_its_own]).unwrap_err();
    assert!(why.contains("cannot hold a kernel for `np.add`"), "{why}");

    // A symbol whose name has no dialect.
    let unnamed: Array = nary::symbol("nameless", [pos]);
    let why = np::get(unnamed, &[np::dialect()]).unwrap_err();
    assert!(why.contains("`nameless` is not"), "{why}");
}

#[test]
fn a_dialect_is_printed_as_what_it_holds() {
    let printed = toy::dialect().to_string();
    assert!(
        printed.contains(
            "equal   toy.euler_step(?0, ?1, ?2) = np.add(?0, np.mul(?2, np.div(np.neg(?0), ?1)))"
        ),
        "{printed}"
    );
    assert!(printed.contains("kernel  toy.euler_step"), "{printed}");
}

#[test]
fn an_equivalence_that_uses_its_nodes_many_times_is_mapped() {
    // Forty steps as one symbol. Each step uses the one before it twice, so
    // written out in full its meaning would be longer than could be read.
    let forty = |pos: Array, mass: Array, dt: Array| {
        let mut at = pos;
        for _ in 0..40 {
            at = step_in_np(at, mass, dt);
        }
        at
    };
    let long = nary::Dialect::new("long")
        .equal(|[pos, mass, dt]: [Array; 3]| {
            (
                nary::symbol("long.forty", [pos, mass, dt]),
                forty(pos, mass, dt),
            )
        })
        .kernel("long.forty", |x: &[&np::Value]| x[0].clone());
    let (pos, mass) = two_arrays();
    let plan = mapped(forty(pos, mass, np::constant(0.5)), &[np::dialect(), long]);
    assert!(plan.contains("long.forty"), "{plan}");
    assert_eq!(plan.lines().count(), 4, "{plan}");
}

#[test]
fn a_hundred_steps_are_mapped_and_computed() {
    let (pos, mass) = two_arrays();
    let dt = np::constant(0.5);
    let mut at = pos;
    for _ in 0..100 {
        at = step_in_np(at, mass, dt);
    }
    // In np alone: four symbols for each step, and the three it starts from.
    let with_np = mapped(at, &[np::dialect()]);
    assert_eq!(with_np.lines().count(), 403, "{with_np}");

    // Given toy as well: one symbol for each step.
    let dialects = [np::dialect(), toy::dialect()];
    let with_md = mapped(at, &dialects);
    assert_eq!(with_md.matches("toy.euler_step").count(), 100, "{with_md}");
    assert_eq!(with_md.lines().count(), 103, "{with_md}");

    assert_eq!(np::get(at, &dialects), np::get(at, &[np::dialect()]));
}
