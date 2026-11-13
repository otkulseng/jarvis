use crate::Value;

/// `np.array` is given which of the values that a program handed over it is.
pub fn array(which: &str, _: &[&Value]) -> Value {
    let which: usize = which.parse().expect("np.array is given a number");
    crate::handed_over(which)
}
