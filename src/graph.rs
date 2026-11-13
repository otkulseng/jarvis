//! A graph as a value: what a node stands for, and what mapping returns.

use crate::{applied, Node};
use std::collections::HashMap;
use std::fmt;

/// One symbol of a graph, and which earlier steps are its inputs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub symbol: String,
    pub attrs: String,
    pub inputs: Vec<usize>,
}

/// Some nodes and all they are made of: the steps, each after its inputs,
/// and which of them are the nodes that were asked for. What two of the
/// nodes share is in the graph once.
///
/// Printed, a graph is one line for each step:
///
/// ```text
/// %0 = np.random.uniform[…]()
/// %1 = np.neg(%0)
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Graph {
    pub steps: Vec<Step>,
    /// The steps that were asked for, in the order they were asked for.
    pub results: Vec<usize>,
}

impl fmt::Display for Graph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, step) in self.steps.iter().enumerate() {
            let inputs: Vec<String> = step.inputs.iter().map(|j| format!("%{j}")).collect();
            let attrs = match step.attrs.as_str() {
                "" => String::new(),
                attrs => format!("[{attrs}]"),
            };
            writeln!(f, "%{i} = {}{attrs}({})", step.symbol, inputs.join(", "))?;
        }
        Ok(())
    }
}

/// The graph a node stands for.
pub fn graph(node: impl Into<Node>) -> Graph {
    graph_all([node])
}

/// The one graph that several nodes stand for.
pub fn graph_all(nodes: impl IntoIterator<Item = impl Into<Node>>) -> Graph {
    let roots: Vec<Node> = nodes.into_iter().map(Into::into).collect();
    let mut steps = Vec::new();
    let mut at: HashMap<Node, usize> = HashMap::new();
    // Each node after its inputs, without recursion: a graph can be deep.
    let mut waiting: Vec<Node> = roots.iter().rev().copied().collect();
    while let Some(&node) = waiting.last() {
        if at.contains_key(&node) {
            waiting.pop();
            continue;
        }
        let made = applied(node);
        let missing: Vec<Node> = made
            .inputs
            .iter()
            .filter(|x| !at.contains_key(x))
            .copied()
            .collect();
        if !missing.is_empty() {
            waiting.extend(missing);
            continue;
        }
        steps.push(Step {
            symbol: made.symbol,
            attrs: made.attrs,
            inputs: made.inputs.iter().map(|x| at[x]).collect(),
        });
        at.insert(node, steps.len() - 1);
        waiting.pop();
    }
    let results = roots.iter().map(|root| at[root]).collect();
    Graph { steps, results }
}
