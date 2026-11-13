use super::each;
use crate::Value;

pub fn add(x: &[&Value]) -> Value {
    each("np.add", x[0], x[1], |a, b| a + b)
}
