use super::each;
use crate::Value;

pub fn div(x: &[&Value]) -> Value {
    each("np.div", x[0], x[1], |a, b| a / b)
}
