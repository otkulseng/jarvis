use super::each;
use crate::Value;

pub fn sub(x: &[&Value]) -> Value {
    each("np.sub", x[0], x[1], |a, b| a - b)
}
