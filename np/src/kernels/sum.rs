use crate::Value;

/// `np.sum` is given the axis it adds up along. That axis is gone from the
/// result.
pub fn sum(axis: &str, x: &[&Value]) -> Value {
    let x = x[0];
    let axis: usize = axis.parse().expect("np.sum is given an axis");
    assert!(
        axis < x.shape.len(),
        "np.sum: an array of shape {:?} has no axis {axis}",
        x.shape
    );
    let outer: usize = x.shape[..axis].iter().product();
    let along = x.shape[axis];
    let inner: usize = x.shape[axis + 1..].iter().product();
    let mut data = vec![0.0; outer * inner];
    for o in 0..outer {
        let into = &mut data[o * inner..(o + 1) * inner];
        for j in 0..along {
            let from = &x.data[(o * along + j) * inner..(o * along + j + 1) * inner];
            for (sum, x) in into.iter_mut().zip(from) {
                *sum += x;
            }
        }
    }
    let mut shape = x.shape.clone();
    shape.remove(axis);
    Value { shape, data }
}
