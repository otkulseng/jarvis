//! The reference backend: plain Rust.

use crate::{Graph, Value};

pub type Kernel<'a> = Box<dyn Fn(&[&Value]) -> Value + 'a>;

pub fn run(g: &Graph) -> Value {
    let mut vals: Vec<Value> = vec![];
    for node in &g.nodes {
        let v = if let Some(k) = node.input {
            g.inputs[k].clone()
        } else {
            let ins: Vec<&Value> = node.inputs.iter().map(|i| &vals[*i]).collect();
            let kernel = node.op.cpu().unwrap_or_else(|| panic!("no cpu kernel for {}", node.key));
            kernel(&ins)
        };
        vals.push(v);
    }
    vals.swap_remove(g.root)
}
