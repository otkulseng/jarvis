//! Running a graph: its kernels are called, each after its inputs.

use crate::dialect::sound;
use crate::{graph, map, Dialect, Graph, Node};
use std::any::Any;

/// Call the kernel of each step on the results of its inputs, and return
/// the result of the last. When several of the dialects have a kernel for a
/// symbol, the first of them is used.
///
/// nary does not look at the results: each kernel is given what the kernels
/// before it returned.
pub fn run(graph: &Graph, dialects: &[Dialect]) -> Result<Box<dyn Any>, String> {
    sound(dialects)?;
    let mut results: Vec<Box<dyn Any>> = Vec::with_capacity(graph.steps.len());
    for step in &graph.steps {
        let code = dialects
            .iter()
            .find_map(|d| d.kernels.get(&step.symbol))
            .ok_or_else(|| {
                format!(
                    "`{}` has no kernel in the dialects given, so the graph cannot be run",
                    step.symbol
                )
            })?;
        let inputs: Vec<&dyn Any> = step.inputs.iter().map(|i| results[*i].as_ref()).collect();
        results.push(code(&step.attrs, &inputs)?);
    }
    results
        .pop()
        .ok_or_else(|| "there is nothing in the graph to run".to_string())
}

/// Compute a node: its graph is mapped onto the kernels of the dialects,
/// they are run, and the result is returned as a `V`. It is an error if
/// the kernels computed something else.
///
/// ```ignore
/// let positions: np::Value = nary::get(pos, &dialects)?;
/// ```
pub fn get<V: 'static>(node: impl Into<Node>, dialects: &[Dialect]) -> Result<V, String> {
    let mapped = map(&graph(node), dialects)?;
    match run(&mapped, dialects)?.downcast::<V>() {
        Ok(result) => Ok(*result),
        Err(_) => Err(format!(
            "the kernels computed something that is not a `{}`",
            std::any::type_name::<V>()
        )),
    }
}
