//! Random numbers as mathematics: an array of them is a function of a key,
//! so the same key always gives the same numbers, and a program says which
//! key each array uses.

use crate::Array;

/// Where random numbers come from. Each use gives the next key back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key(u64);

pub fn from_seed(seed: u64) -> Key {
    Key(mix(seed))
}

/// Numbers spread evenly over [0, 1), and the key to use next.
pub fn uniform(key: Key, shape: Vec<usize>) -> (Array, Key) {
    let shape: Vec<String> = shape.iter().map(|n| n.to_string()).collect();
    let key_and_shape = format!("{} {}", key.0, shape.join(" "));
    (
        nary::symbol_with("np.random.uniform", key_and_shape, nary::NONE),
        Key(mix(key.0)),
    )
}

/// One step of splitmix64.
pub(crate) fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
