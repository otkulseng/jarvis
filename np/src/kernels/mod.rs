//! The kernels of np: one file for each symbol.
//!
//! A kernel is a function from the computed inputs of a symbol to its
//! result. One for a symbol that is given attributes takes them first.
//!
//! Two arrays are put together element by element as in NumPy (`each.rs`):
//! an input with no shape is a number, and stands for every element of the
//! other; so does an axis of length one, along that axis.

mod add;
mod array;
mod constant;
mod div;
mod each;
mod expand_dims;
mod less;
mod mul;
mod neg;
mod round;
mod sub;
mod sum;
mod uniform;

pub use add::add;
pub use array::array;
pub use constant::constant;
pub use div::div;
use each::each;
pub use expand_dims::expand_dims;
pub use less::less;
pub use mul::mul;
pub use neg::neg;
pub use round::round;
pub use sub::sub;
pub use sum::sum;
pub use uniform::uniform;
