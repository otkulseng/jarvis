use crate::Value;

pub fn sub(x: &[&Value]) -> Value {
    let (a, b) = (x[0], x[1]);
    if a.shape.is_empty() {
        let a = a.data[0];
        return Value {
            shape: b.shape.clone(),
            data: b.data.iter().map(|b| a - b).collect(),
        };
    }
    if b.shape.is_empty() {
        let b = b.data[0];
        return Value {
            shape: a.shape.clone(),
            data: a.data.iter().map(|a| a - b).collect(),
        };
    }
    Value {
        shape: a.shape.clone(),
        data: a.data.iter().zip(&b.data).map(|(a, b)| a - b).collect(),
    }
}
