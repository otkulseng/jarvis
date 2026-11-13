//! nary: dialects, equivalences between them, and the mapping of a graph
//! onto kernels.
//!
//! A **symbol** belongs to a dialect and is named `dialect.name`. A **node**
//! is a symbol applied to nodes. That is all nary knows about a node: not
//! what it stands for, not how it is computed.
//!
//! A **dialect** is a value. It holds two things, both optional:
//!
//! - **equivalences**: an expression in its symbols is the same as an
//!   expression in another dialect's symbols;
//! - **kernels**: code that computes one of its symbols. nary does not look
//!   at what a kernel takes or returns; that is between the kernels.
//!
//! To **map** a graph, given some dialects, is to work out every form the
//! graph is equal to and to pick one made of symbols that have kernels. To
//! **run** a mapped graph is to call those kernels, each after its inputs.
//!
//! nary is not a compiler. It does not optimize or generate code: all code
//! that computes is in kernels, and nary only maps a graph onto them.
//!
//! No dialect is special. A dialect of arrays (`np/`) and a dialect with
//! one symbol for a whole simulation step are the same kind of thing.
//!
//! ```text
//!   lib.rs      nodes and symbols
//!   dialect.rs  Dialect: equivalences and kernels
//!   graph.rs    Graph: a node and all it is made of, as a value
//!   map.rs      map, through egglog
//!   run.rs      run, and get: all of it at once
//! ```
//!
//! Nothing here panics on what it is given. What is wrong with a dialect
//! is kept with it, and `map`, `run` and `get` return it as their error.

mod dialect;
mod graph;
mod map;
mod run;

pub use dialect::Dialect;
pub use graph::{graph, graph_all, Graph, Step};
pub use map::map;
pub use run::{get, get_all, run};

use std::cell::RefCell;
use std::collections::HashMap;

/// A place in a graph. It is small and copied freely; `graph` gives the
/// graph it stands for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Node(u32);

/// No inputs: for a symbol that is applied to nothing.
pub const NONE: [Node; 0] = [];

/// A symbol applied to nodes. `attrs` is what the symbol is given that is
/// not a node, as text: its dialect decides what it says.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Applied {
    symbol: String,
    attrs: String,
    inputs: Vec<Node>,
}

/// The symbol of a node that stands for "any node" in an equivalence.
const ANY: &str = "?";

/// Every node that was made. The same symbol on the same inputs is the
/// same node.
#[derive(Default)]
struct Store {
    nodes: Vec<Applied>,
    ids: HashMap<Applied, Node>,
}

thread_local!(static STORE: RefCell<Store> = RefCell::default());

fn make(applied: Applied) -> Node {
    STORE.with(|store| {
        let mut store = store.borrow_mut();
        if let Some(node) = store.ids.get(&applied) {
            return *node;
        }
        let node = Node(store.nodes.len() as u32);
        store.nodes.push(applied.clone());
        store.ids.insert(applied, node);
        node
    })
}

fn applied(node: Node) -> Applied {
    STORE.with(|store| store.borrow().nodes[node.0 as usize].clone())
}

/// The dialect of a symbol: what comes before the first `.` of its name.
/// None for a name that is not `dialect.name`.
fn dialect_of(symbol: &str) -> Option<&str> {
    match symbol.split_once('.') {
        Some((dialect, name)) if !dialect.is_empty() && !name.is_empty() => Some(dialect),
        _ => None,
    }
}

/// The symbol `dialect.name` applied to `inputs`.
///
/// The inputs and the result are whatever a dialect uses for its nodes:
/// any type that can be made from a `Node` and turned back into one.
///
/// ```ignore
/// pub fn neg(x: Array) -> Array {
///     nary::symbol("np.neg", [x])
/// }
/// ```
pub fn symbol<T: From<Node>>(name: &str, inputs: impl IntoIterator<Item = impl Into<Node>>) -> T {
    symbol_with(name, "", inputs)
}

/// A symbol that is also given something that is not a node: a number, a
/// size, a key. A name that is not `dialect.name` is reported by `map`.
pub fn symbol_with<T: From<Node>>(
    name: &str,
    attrs: impl ToString,
    inputs: impl IntoIterator<Item = impl Into<Node>>,
) -> T {
    make(Applied {
        symbol: name.into(),
        attrs: attrs.to_string(),
        inputs: inputs.into_iter().map(Into::into).collect(),
    })
    .into()
}
