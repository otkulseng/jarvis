//! Running a graph: its kernels are called, each after its inputs.

use crate::dialect::sound;
use crate::{graph, graph_all, map, Dialect, Graph, Node};
use std::any::Any;
use std::rc::Rc;

/// Call the kernel of each step on the results of its inputs, and return
/// the results that the graph was asked for, in that order. When several of
/// the dialects have a kernel for a symbol, the first of them is used.
///
/// nary does not look at the results: each kernel is given what the kernels
/// before it returned. A result is let go of as soon as nothing after it
/// needs it.
pub fn run(graph: &Graph, dialects: &[Dialect]) -> Result<Vec<Rc<dyn Any>>, String> {
    sound(dialects)?;
    if graph.results.is_empty() {
        return Err("there is nothing in the graph to run".into());
    }
    // How many times each result is still needed.
    let mut needed = vec![0usize; graph.steps.len()];
    let uses = graph.steps.iter().flat_map(|step| &step.inputs);
    for used in uses.chain(&graph.results) {
        *needed.get_mut(*used).ok_or_else(|| {
            format!("the graph uses its step {used}, and has no such step")
        })? += 1;
    }
    let mut results: Vec<Option<Rc<dyn Any>>> = Vec::with_capacity(graph.steps.len());
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
        let inputs: Option<Vec<&dyn Any>> = step
            .inputs
            .iter()
            .map(|i| results.get(*i).and_then(|r| r.as_deref()))
            .collect();
        let inputs = inputs.ok_or_else(|| {
            format!("`{}` comes before one of its inputs in the graph", step.symbol)
        })?;
        let result = code(&step.attrs, &inputs)?;
        for used in &step.inputs {
            needed[*used] -= 1;
            if needed[*used] == 0 {
                results[*used] = None;
            }
        }
        results.push(Some(Rc::from(result)));
    }
    Ok(graph
        .results
        .iter()
        .filter_map(|asked| results[*asked].clone())
        .collect())
}

/// Compute a node: its graph is mapped onto the kernels of the dialects,
/// they are run, and the result is returned as a `V`. It is an error if
/// the kernels computed something else.
///
/// ```ignore
/// let positions: np::Value = nary::get(pos, &dialects)?;
/// ```
pub fn get<V: 'static + Clone>(node: impl Into<Node>, dialects: &[Dialect]) -> Result<V, String> {
    let mapped = map(&graph(node), dialects)?;
    typed(run(&mapped, dialects)?)?
        .pop()
        .ok_or_else(|| "nothing was computed".to_string())
}

/// Compute several nodes at once, as one graph: what they share is computed
/// once. The results are in the order of the nodes.
///
/// ```ignore
/// let both: Vec<np::Value> = nary::get_all([pos, vel], &dialects)?;
/// ```
pub fn get_all<V: 'static + Clone>(
    nodes: impl IntoIterator<Item = impl Into<Node>>,
    dialects: &[Dialect],
) -> Result<Vec<V>, String> {
    let mapped = map(&graph_all(nodes), dialects)?;
    typed(run(&mapped, dialects)?)
}

/// Results as `V`s. One that two nodes share is copied for the second.
fn typed<V: 'static + Clone>(results: Vec<Rc<dyn Any>>) -> Result<Vec<V>, String> {
    results
        .into_iter()
        .map(|result| match result.downcast::<V>() {
            Ok(result) => Ok(Rc::unwrap_or_clone(result)),
            Err(_) => Err(format!(
                "the kernels computed something that is not a `{}`",
                std::any::type_name::<V>()
            )),
        })
        .collect()
}
