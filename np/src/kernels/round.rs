use crate::Value;

/// The nearest whole number, and the one further from zero when two are
/// equally near.
pub fn round(x: &[&Value]) -> Value {
    Value {
        shape: x[0].shape.clone(),
        data: x[0].data.iter().map(|a| a.round()).collect(),
    }
}
