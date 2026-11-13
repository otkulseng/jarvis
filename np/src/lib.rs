//! np: arrays, on top of nary.
//!
//! This file is all of np that a program uses: an array, the operations on
//! arrays, the dialect, and `get`. The code that computes them is in
//! `kernels/`.

mod kernels;
pub mod random;

use nary::{symbol, symbol_with, Dialect, Node};
use std::cell::RefCell;

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

/// The nearest whole number to each element.
pub fn round(x: Array) -> Array {
    symbol("np.round", [x])
}

/// One where `x` is less than `y`, zero elsewhere.
pub fn less(x: Array, y: Array) -> Array {
    symbol("np.less", [x, y])
}

/// The sum along an axis, which is then gone from the shape.
pub fn sum(x: Array, axis: usize) -> Array {
    symbol_with("np.sum", axis, [x])
}

/// The same array with one more axis, of length one, at `axis`.
pub fn expand_dims(x: Array, axis: usize) -> Array {
    symbol_with("np.expand_dims", axis, [x])
}

thread_local!(static HANDED_OVER: RefCell<Vec<Value>> = RefCell::default());

/// A value the program has, as an array: where a graph begins from what
/// Rust computed or read. The value is kept for as long as the thread runs.
pub fn array(value: Value) -> Array {
    let which = HANDED_OVER.with(|all| {
        let mut all = all.borrow_mut();
        all.push(value);
        all.len() - 1
    });
    symbol_with("np.array", which, nary::NONE)
}

/// One of the values given to `array`.
pub(crate) fn handed_over(which: usize) -> Value {
    HANDED_OVER.with(|all| all.borrow()[which].clone())
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
        .kernel("np.round", kernels::round)
        .kernel("np.less", kernels::less)
        .kernel_with("np.sum", kernels::sum)
        .kernel_with("np.expand_dims", kernels::expand_dims)
        .kernel_with("np.array", kernels::array)
        .kernel_with("np.const", kernels::constant)
        .kernel_with("np.random.uniform", kernels::uniform)
}

/// Compute an array, with the kernels of these dialects: what `nary::get`
/// computes, as the `Value` an array is.
pub fn get(x: Array, dialects: &[Dialect]) -> Result<Value, String> {
    nary::get(x, dialects)
}

/// Compute several arrays at once, as one graph: what they share is
/// computed once. The values are in the order of the arrays.
pub fn get_all(x: &[Array], dialects: &[Dialect]) -> Result<Vec<Value>, String> {
    nary::get_all(x.iter().copied(), dialects)
}
