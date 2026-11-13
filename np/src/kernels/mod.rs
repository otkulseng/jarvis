//! The kernels of np: one file for each symbol.
//!
//! A kernel is a function from the computed inputs of a symbol to its
//! result. One for a symbol that is given attributes takes them first.
//!
//! An input with no shape is a number, and stands for every element of the
//! other input.

mod add;
mod constant;
mod div;
mod mul;
mod neg;
mod sub;
mod uniform;

pub use add::add;
pub use constant::constant;
pub use div::div;
pub use mul::mul;
pub use neg::neg;
pub use sub::sub;
pub use uniform::uniform;
