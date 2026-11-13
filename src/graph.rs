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

/// A node and all it is made of: its steps, each after its inputs. The last
/// step is the node itself.
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
    let root = node.into();
    let mut steps = Vec::new();
    let mut at: HashMap<Node, usize> = HashMap::new();
    // Each node after its inputs, without recursion: a graph can be deep.
    let mut waiting = vec![root];
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
    Graph { steps }
}
