use super::each;
use crate::Value;

pub fn mul(x: &[&Value]) -> Value {
    each("np.mul", x[0], x[1], |a, b| a * b)
}
