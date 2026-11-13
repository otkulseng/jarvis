//! See DESIGN.md. A program builds a graph of operations. `run` puts the graph
//! and every operation's equivalent in an e-graph, extracts the cheapest form
//! in which every node has a kernel on the chosen backend, and runs it there.
//!
//! The graph is one per thread. An `Array` is a copyable handle into it.

use ::std::cell::RefCell;
use ::std::collections::{BTreeMap, HashMap};
use ::std::fmt::Debug;
use ::std::rc::Rc;

use egglog::{CommandOutput, EGraph, Term, TermDag, TermId};

pub mod cpu;
pub mod cubecl;
pub mod stablehlo;
pub mod std;

#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    F32(Vec<f32>),
    I32(Vec<i32>),
}

/// Numbers with a shape: what goes in and what comes out.
#[derive(Clone, Debug, PartialEq)]
pub struct Value {
    pub shape: Vec<usize>,
    pub data: Data,
}

impl From<Vec<f32>> for Data {
    fn from(v: Vec<f32>) -> Data {
        Data::F32(v)
    }
}
impl From<Vec<i32>> for Data {
    fn from(v: Vec<i32>) -> Data {
        Data::I32(v)
    }
}

impl Value {
    pub fn new<E>(shape: &[usize], data: Vec<E>) -> Value
    where
        Vec<E>: Into<Data>,
    {
        let data = data.into();
        let len = match &data {
            Data::F32(v) => v.len(),
            Data::I32(v) => v.len(),
        };
        assert_eq!(len, shape.iter().product::<usize>());
        Value { shape: shape.to_vec(), data }
    }
    /// The numbers as Rust f32. The device dtype must be f32.
    pub fn f32(&self) -> &[f32] {
        match &self.data {
            Data::F32(v) => v,
            Data::I32(_) => panic!("expected f32"),
        }
    }
    pub fn i32(&self) -> &[i32] {
        match &self.data {
            Data::I32(v) => v,
            Data::F32(_) => panic!("expected i32"),
        }
    }
}

/// A nested array literal: `[[1.0, 2.0], [3.0, 4.0]]` is a 2x2 f32 array.
pub trait Nested {
    type Elem: Elem;
    fn shape() -> Vec<usize>;
    fn push(&self, out: &mut Vec<Self::Elem>);
}
pub trait Elem: Copy {
    fn data(v: Vec<Self>) -> Data;
}
impl Elem for f32 {
    fn data(v: Vec<f32>) -> Data {
        Data::F32(v)
    }
}
impl Elem for f64 {
    fn data(v: Vec<f64>) -> Data {
        Data::F32(v.into_iter().map(|x| x as f32).collect())
    }
}
impl Elem for i32 {
    fn data(v: Vec<i32>) -> Data {
        Data::I32(v)
    }
}
impl<E: Elem> Nested for E {
    type Elem = E;
    fn shape() -> Vec<usize> {
        vec![]
    }
    fn push(&self, out: &mut Vec<E>) {
        out.push(*self)
    }
}
impl<T: Nested, const N: usize> Nested for [T; N] {
    type Elem = T::Elem;
    fn shape() -> Vec<usize> {
        let mut s = vec![N];
        s.extend(T::shape());
        s
    }
    fn push(&self, out: &mut Vec<T::Elem>) {
        self.iter().for_each(|t| t.push(out))
    }
}

/// An operation of a dialect. A dialect is an enum of these. `Debug` names the
/// operation and its attributes. Everything but `dialect` and `shape` is
/// optional: implement what the operation has.
pub trait Op: Debug + 'static {
    fn dialect(&self) -> &'static str;
    fn shape(&self, ins: &[&[usize]]) -> Vec<usize>;
    /// The same thing written with other operations. A backend without a
    /// kernel for this operation runs that instead.
    fn equivalent(&self, _ins: &[Array]) -> Option<Array> {
        None
    }
    fn cpu(&self) -> Option<cpu::Kernel<'_>> {
        None
    }
    fn stablehlo(&self) -> Option<stablehlo::Kernel<'_>> {
        None
    }
    fn cubecl(&self) -> Option<cubecl::Kernel<'_>> {
        None
    }
}

/// A node names a kernel: an operation and its inputs. An input node has
/// `input` set to its index.
#[derive(Clone)]
pub struct Node {
    pub op: Rc<dyn Op>,
    pub key: String,
    pub input: Option<usize>,
    pub inputs: Vec<usize>,
    pub shape: Vec<usize>,
}

/// What a backend receives: nodes in dependency order, the inputs, the root.
pub struct Graph {
    pub nodes: Vec<Node>,
    pub inputs: Vec<Value>,
    pub root: usize,
}

/// Where a graph runs: which kernels, and which runtime launches them. A
/// backend's device, such as which GPU, is the backend's own environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    StableHlo,
    CubeCl,
}

impl Backend {
    /// From `NARY_BACKEND`: `cpu`, `stablehlo` or `cubecl`. Unset is the CPU.
    pub fn from_env() -> Backend {
        match ::std::env::var("NARY_BACKEND").as_deref() {
            Err(_) | Ok("cpu") => Backend::Cpu,
            Ok("stablehlo") => Backend::StableHlo,
            Ok("cubecl") => Backend::CubeCl,
            Ok(other) => panic!("no backend {other}"),
        }
    }
}

/// A handle to a node of this thread's graph. Operations on it add nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Array {
    id: usize,
}

struct Ctx {
    nodes: Vec<Node>,
    inputs: Vec<Value>,
    seen: HashMap<(String, Vec<usize>), usize>,
}

thread_local! {
    static CTX: RefCell<Ctx> = RefCell::new(Ctx { nodes: vec![], inputs: vec![], seen: HashMap::new() });
}

#[derive(Debug)]
struct In(usize);
impl Op for In {
    fn dialect(&self) -> &'static str {
        "nary"
    }
    fn shape(&self, _: &[&[usize]]) -> Vec<usize> {
        unreachable!()
    }
}

/// An input, from a nested array literal.
pub fn array<N: Nested>(x: N) -> Array {
    let mut v = vec![];
    x.push(&mut v);
    input(Value { shape: N::shape(), data: N::Elem::data(v) })
}

pub fn input(v: Value) -> Array {
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        let k = c.inputs.len();
        c.inputs.push(v.clone());
        let op = In(k);
        c.nodes.push(Node { key: key(&op), op: Rc::new(op), input: Some(k), inputs: vec![], shape: v.shape });
        Array { id: c.nodes.len() - 1 }
    })
}

/// Adds a node of `op` over `ins`, or finds the node that already is that.
/// Dialects build their frontends on this.
pub fn apply(op: impl Op, ins: &[Array]) -> Array {
    let key = key(&op);
    let ids: Vec<usize> = ins.iter().map(|a| a.id).collect();
    CTX.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(&id) = c.seen.get(&(key.clone(), ids.clone())) {
            return Array { id };
        }
        let shapes: Vec<&[usize]> = ids.iter().map(|i| c.nodes[*i].shape.as_slice()).collect();
        let shape = op.shape(&shapes);
        c.nodes.push(Node { op: Rc::new(op), key: key.clone(), input: None, inputs: ids.clone(), shape });
        let id = c.nodes.len() - 1;
        c.seen.insert((key, ids), id);
        Array { id }
    })
}

/// The identity of an operation with its attributes, as an egglog symbol.
fn key(op: &dyn Op) -> String {
    format!("{}.{:?}", op.dialect(), op)
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '(' => '[',
            ')' => ']',
            ';' => ',',
            c => c,
        })
        .collect()
}

impl Array {
    pub fn shape(self) -> Vec<usize> {
        CTX.with(|c| c.borrow().nodes[self.id].shape.clone())
    }

    /// The numbers, from the backend `NARY_BACKEND` names, or the CPU.
    pub fn get(self) -> Value {
        self.get_on(Backend::from_env())
    }

    pub fn get_on(self, backend: Backend) -> Value {
        let g = lower(self.id, backend);
        match backend {
            Backend::Cpu => cpu::run(&g),
            Backend::StableHlo => stablehlo::run(&g),
            Backend::CubeCl => cubecl::run(&g),
        }
    }
}

/// Assert every node's equivalent in an e-graph, and extract the cheapest
/// graph whose nodes all have a kernel on `backend`.
fn lower(root: usize, backend: Backend) -> Graph {
    // Each node's equivalent, including the nodes that adds.
    let mut unions = vec![];
    let mut i = 0;
    while let Some((op, ins)) = CTX.with(|c| {
        let c = c.borrow();
        let n = c.nodes.get(i)?;
        Some((n.op.clone(), n.inputs.iter().map(|id| Array { id: *id }).collect::<Vec<_>>()))
    }) {
        if let Some(eq) = op.equivalent(&ins) {
            unions.push((i, eq.id));
        }
        i += 1;
    }

    CTX.with(|c| {
        let c = c.borrow();
        let cost = |n: &Node| {
            let has = match backend {
                _ if n.input.is_some() => return 0,
                Backend::Cpu => n.op.cpu().is_some(),
                Backend::StableHlo => n.op.stablehlo().is_some(),
                Backend::CubeCl => n.op.cubecl().is_some(),
            };
            if has { 1 } else { 1_000_000 }
        };
        let mut ctors = BTreeMap::new();
        let mut by_key = HashMap::new();
        for (i, n) in c.nodes.iter().enumerate() {
            ctors.insert(&n.key, format!("({} {}:cost {})", n.key, "T ".repeat(n.inputs.len()), cost(n)));
            by_key.entry(&n.key).or_insert(i);
        }
        let mut p = format!("(datatype T {})\n", ctors.values().cloned().collect::<Vec<_>>().join(" "));
        for (i, n) in c.nodes.iter().enumerate() {
            let ins: Vec<String> = n.inputs.iter().map(|j| format!(" n{j}")).collect();
            p += &format!("(let n{i} ({}{}))\n", n.key, ins.concat());
        }
        for (a, b) in &unions {
            p += &format!("(union n{a} n{b})\n");
        }
        p += &format!("(extract n{root})\n");

        let mut egraph = EGraph::default();
        let outputs = egraph.parse_and_run_program(None, &p).unwrap_or_else(|e| panic!("egglog: {e}\n{p}"));
        let (dag, best) = outputs
            .iter()
            .find_map(|o| match o {
                CommandOutput::ExtractBest(dag, _, id) => Some((dag, *id)),
                _ => None,
            })
            .expect("egglog extracted nothing");

        let mut nodes = vec![];
        let root = rebuild(&c.nodes, &by_key, dag, best, &mut nodes, &mut HashMap::new());
        Graph { nodes, inputs: c.inputs.clone(), root }
    })
}

fn rebuild(
    orig: &[Node],
    by_key: &HashMap<&String, usize>,
    dag: &TermDag,
    id: TermId,
    nodes: &mut Vec<Node>,
    memo: &mut HashMap<TermId, usize>,
) -> usize {
    if let Some(&n) = memo.get(&id) {
        return n;
    }
    let Term::App(key, kids) = dag.get(id) else { panic!("unexpected term {:?}", dag.get(id)) };
    let mut node = orig[by_key[key]].clone();
    node.inputs = kids.iter().map(|k| rebuild(orig, by_key, dag, *k, nodes, memo)).collect();
    if node.input.is_none() {
        let shapes: Vec<&[usize]> = node.inputs.iter().map(|i| nodes[*i].shape.as_slice()).collect();
        node.shape = node.op.shape(&shapes);
    }
    nodes.push(node);
    memo.insert(id, nodes.len() - 1);
    nodes.len() - 1
}
