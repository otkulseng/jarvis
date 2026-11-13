use super::each;
use crate::Value;

/// One where the first is less than the second, zero elsewhere.
pub fn less(x: &[&Value]) -> Value {
    each("np.less", x[0], x[1], |a, b| if a < b { 1.0 } else { 0.0 })
}
