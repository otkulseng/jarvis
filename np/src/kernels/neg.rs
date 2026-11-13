use crate::Value;

pub fn neg(x: &[&Value]) -> Value {
    Value {
        shape: x[0].shape.clone(),
        data: x[0].data.iter().map(|a| -a).collect(),
    }
}
