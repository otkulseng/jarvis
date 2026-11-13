//! The arrays of np: shapes that fit together, sums, and several results.

use np::{Array, Value};

fn array(shape: &[usize], data: &[f64]) -> Array {
    np::array(Value {
        shape: shape.to_vec(),
        data: data.to_vec(),
    })
}

fn get(x: Array) -> Value {
    np::get(x, &[np::dialect()]).unwrap()
}

#[test]
fn an_axis_of_length_one_stands_for_every_element_along_it() {
    let column = array(&[2, 1], &[10.0, 20.0]);
    let row = array(&[3], &[1.0, 2.0, 3.0]);
    let both = get(np::add(column, row));
    assert_eq!(both.shape, [2, 3]);
    assert_eq!(both.data, [11.0, 12.0, 13.0, 21.0, 22.0, 23.0]);
}

#[test]
fn every_pair_of_rows_is_made_with_two_new_axes() {
    let x = array(&[2, 2], &[0.0, 0.0, 1.0, 3.0]);
    let apart = get(np::sub(np::expand_dims(x, 1), np::expand_dims(x, 0)));
    assert_eq!(apart.shape, [2, 2, 2]);
    // The first row less the second, and the second less the first.
    assert_eq!(apart.data, [0.0, 0.0, -1.0, -3.0, 1.0, 3.0, 0.0, 0.0]);
}

#[test]
fn a_sum_is_along_one_axis() {
    let x = array(&[2, 3], &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let down = get(np::sum(x, 0));
    assert_eq!((down.shape, down.data), (vec![3], vec![5.0, 7.0, 9.0]));
    let across = get(np::sum(x, 1));
    assert_eq!((across.shape, across.data), (vec![2], vec![6.0, 15.0]));
    let all = get(np::sum(np::sum(x, 1), 0));
    assert_eq!((all.shape, all.data), (vec![], vec![21.0]));
}

#[test]
fn round_and_less() {
    let x = array(&[4], &[-0.6, -0.4, 0.4, 1.6]);
    assert_eq!(get(np::round(x)).data, [-1.0, -0.0, 0.0, 2.0]);
    assert_eq!(get(np::less(x, np::constant(0.0))).data, [1.0, 1.0, 0.0, 0.0]);
}

#[test]
fn several_arrays_are_computed_as_one_graph() {
    let x = array(&[2], &[1.0, 2.0]);
    let shared = np::mul(x, x);
    let (sum, difference) = (np::add(shared, x), np::sub(shared, x));

    // What both are made of is in the graph once.
    let both = nary::graph_all([sum, difference]);
    let mapped = nary::map(&both, &[np::dialect()]).unwrap();
    assert_eq!(mapped.to_string().matches("np.mul").count(), 1, "{mapped}");
    assert_eq!(mapped.results.len(), 2);

    let values = np::get_all(&[sum, difference, sum], &[np::dialect()]).unwrap();
    assert_eq!(values[0].data, [2.0, 6.0]);
    assert_eq!(values[1].data, [0.0, 2.0]);
    assert_eq!(values[2], values[0]);
}
