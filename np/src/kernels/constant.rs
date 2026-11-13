use crate::Value;

/// `np.const` is given its number.
pub fn constant(number: &str, _: &[&Value]) -> Value {
    Value {
        shape: vec![],
        data: vec![number.parse().expect("np.const is given a number")],
    }
}
