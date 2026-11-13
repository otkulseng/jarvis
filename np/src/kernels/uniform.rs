use crate::random::mix;
use crate::Value;

/// `np.random.uniform` is given its key and then its shape.
pub fn uniform(key_and_shape: &str, _: &[&Value]) -> Value {
    let mut words = key_and_shape.split(' ');
    let mut state: u64 = words.next().and_then(|w| w.parse().ok()).expect("a key");
    let shape: Vec<usize> = words.map(|w| w.parse().expect("a size")).collect();
    let data = (0..shape.iter().product())
        .map(|_| {
            state = mix(state);
            (state >> 11) as f64 / (1u64 << 53) as f64
        })
        .collect();
    Value { shape, data }
}
