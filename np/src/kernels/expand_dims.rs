use crate::Value;

/// `np.expand_dims` is given where the new axis, of length one, goes.
pub fn expand_dims(axis: &str, x: &[&Value]) -> Value {
    let x = x[0];
    let axis: usize = axis.parse().expect("np.expand_dims is given an axis");
    assert!(
        axis <= x.shape.len(),
        "np.expand_dims: an array of shape {:?} has no place for an axis {axis}",
        x.shape
    );
    let mut shape = x.shape.clone();
    shape.insert(axis, 1);
    Value {
        shape,
        data: x.data.clone(),
    }
}
