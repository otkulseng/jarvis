//! np: arrays, on top of nary.
//!
//! This file is all of np that a program uses: an array, the operations on
//! arrays, the dialect, and `get`. The code that computes them is in
//! `kernels/`.

mod kernels;
pub mod random;

use nary::{symbol, symbol_with, Dialect, Node};

/// An array.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Array(Node);

impl From<Node> for Array {
    fn from(node: Node) -> Array {
        Array(node)
    }
}

impl From<Array> for Node {
    fn from(array: Array) -> Node {
        array.0
    }
}

/// An array that has been computed.
#[derive(Clone, PartialEq, Debug)]
pub struct Value {
    pub shape: Vec<usize>,
    pub data: Vec<f64>,
}

pub fn add(x: Array, y: Array) -> Array {
    symbol("np.add", [x, y])
}

pub fn sub(x: Array, y: Array) -> Array {
    symbol("np.sub", [x, y])
}

pub fn mul(x: Array, y: Array) -> Array {
    symbol("np.mul", [x, y])
}

pub fn div(x: Array, y: Array) -> Array {
    symbol("np.div", [x, y])
}

pub fn neg(x: Array) -> Array {
    symbol("np.neg", [x])
}

/// A number, as an array that fits any shape.
pub fn constant(value: f64) -> Array {
    symbol_with("np.const", value, nary::NONE)
}

/// np: each of its symbols, and the kernel that computes it.
pub fn dialect() -> Dialect {
    Dialect::new("np")
        .kernel("np.add", kernels::add)
        .kernel("np.sub", kernels::sub)
        .kernel("np.mul", kernels::mul)
        .kernel("np.div", kernels::div)
        .kernel("np.neg", kernels::neg)
        .kernel_with("np.const", kernels::constant)
        .kernel_with("np.random.uniform", kernels::uniform)
}

/// Compute an array, with the kernels of these dialects: what `nary::get`
/// computes, as the `Value` an array is.
pub fn get(x: Array, dialects: &[Dialect]) -> Result<Value, String> {
    nary::get(x, dialects)
}
