//! Mapping a graph onto kernels.
//!
//! The graph and the dialects' equivalences go into egglog, which works out
//! every form the graph is equal to. Then one form is picked in which every
//! symbol has a kernel: the one with the fewest.
//!
//! This is all egglog is used for. It does not simplify or optimize the
//! graph. It shows that a form with a kernel for every symbol exists, and it
//! holds what each node is equal to, which is what a derivative will be
//! taken through. "The fewest symbols" is only the choice made for now.
//!
//! In egglog, a symbol with `n` inputs is a table `name/n` with the columns
//! (attributes, input, …, input) → node. An equivalence is a rule for each
//! direction it can be read in. Kernels are not in egglog. A symbol without
//! one in the dialects given has a table that nothing may be picked from
//! (`unextractable`).

use crate::dialect::sound;
use crate::{dialect_of, graph, Dialect, Graph, Node, Step, ANY};
use egglog::ast::*;
use egglog::extract::{CostModel, Extractor};
use egglog::prelude::*;
use egglog::{ArcSort, EGraph, Enode, Function, Term, TermDag, TermId};
use num::BigInt;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

fn table(symbol: &str, inputs: usize) -> String {
    format!("{symbol}/{inputs}")
}

/// One side of an equivalence as egglog reads it in a rule.
///
/// Each of its nodes is given a name and is there once, however many times
/// it is used: a side written as one nested expression would repeat a node
/// that is used twice, and all that it is made of, and so grow without
/// bound.
struct Side {
    /// Each node that is a symbol: its name, and the symbol applied to the
    /// names of its inputs. Each is after its inputs.
    parts: Vec<(String, Expr)>,
    /// The name of the side as a whole.
    whole: Expr,
    /// The names of its `ANY` nodes.
    any: BTreeSet<String>,
}

/// A side of an equivalence, its nodes named `prefix0`, `prefix1`, … and
/// its `ANY` nodes `any0`, `any1`, …, which are the same names on both
/// sides. The tables it uses are noted on the way.
fn side(node: Node, prefix: &str, tables: &mut BTreeMap<String, (String, usize)>) -> Side {
    let steps = graph(node).steps;
    let name = |i: usize| match steps[i].symbol == ANY {
        true => format!("any{}", steps[i].attrs),
        false => format!("{prefix}{i}"),
    };
    let mut parts = Vec::new();
    let mut any = BTreeSet::new();
    for (i, step) in steps.iter().enumerate() {
        if step.symbol == ANY {
            any.insert(name(i));
            continue;
        }
        let of = table(&step.symbol, step.inputs.len());
        tables.insert(of.clone(), (step.symbol.clone(), step.inputs.len()));
        let mut columns = vec![exprs::string(&step.attrs)];
        columns.extend(step.inputs.iter().map(|j| exprs::var(&name(*j))));
        parts.push((name(i), exprs::call(&of, columns)));
    }
    Side {
        parts,
        whole: exprs::var(&name(steps.len() - 1)),
        any,
    }
}

/// What a form costs: one for each symbol in it, counted every time it is
/// used. The numbers have no largest value, so two forms are always told
/// apart, however deep the graph.
struct Fewest;

impl CostModel<BigInt> for Fewest {
    fn fold(&self, _head: &str, inputs: &[BigInt], head: BigInt) -> BigInt {
        inputs.iter().fold(head, |all, cost| all + cost)
    }

    fn enode_cost(&self, _: &EGraph, _: &Function, _: &Enode<'_>) -> BigInt {
        BigInt::from(1)
    }

    // The attributes of a symbol cost nothing.
    fn base_value_cost(&self, _: &EGraph, _: &ArcSort, _: egglog::Value) -> BigInt {
        BigInt::from(0)
    }
}

/// Every form the graph is equal to, by the dialects' equivalences, is
/// worked out, and the one with the fewest symbols, all of which have a
/// kernel in the dialects, is returned.
pub fn map(graph: &Graph, dialects: &[Dialect]) -> Result<Graph, String> {
    map_in(graph, dialects).map_err(|e| e.to_string())
}

fn map_in(graph: &Graph, dialects: &[Dialect]) -> Result<Graph, Box<dyn std::error::Error>> {
    sound(dialects)?;
    if graph.results.is_empty() {
        return Err("there is nothing in the graph to map".into());
    }
    if let Some(step) = graph.results.iter().find(|r| **r >= graph.steps.len()) {
        return Err(format!("the graph was asked for its step {step}, and has no such step").into());
    }
    // A table for each symbol, at each number of inputs it is used with.
    let mut tables: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for step in &graph.steps {
        if step.symbol == ANY {
            return Err(
                "this is one side of an equivalence, not a graph: it stands for any node".into(),
            );
        }
        tables.insert(
            table(&step.symbol, step.inputs.len()),
            (step.symbol.clone(), step.inputs.len()),
        );
    }
    // Each equivalence, in each direction it can be read in.
    let mut rules = Vec::new();
    for (left, right) in dialects.iter().flat_map(|d| &d.equal) {
        let left = Rc::new(side(*left, "left", &mut tables));
        let right = Rc::new(side(*right, "right", &mut tables));
        // A side can be looked for if it is a symbol, and the other side
        // can then be added if all it stands on was found.
        if !left.parts.is_empty() && right.any.is_subset(&left.any) {
            rules.push((left.clone(), right.clone()));
        }
        if !right.parts.is_empty() && left.any.is_subset(&right.any) {
            rules.push((right, left));
        }
    }

    if let Some((symbol, _)) = tables
        .values()
        .find(|(symbol, _)| dialect_of(symbol).is_none())
    {
        return Err(format!("a symbol is named `dialect.name`, and `{symbol}` is not").into());
    }

    let has_kernel = |symbol: &str| dialects.iter().any(|d| d.kernels.contains_key(symbol));
    let mut eg = EGraph::default();
    add_sort(&mut eg, "Node")?;
    for (name, (symbol, inputs)) in &tables {
        let mut input = vec!["String".to_string()];
        input.extend(vec!["Node".to_string(); *inputs]);
        add_constructor(
            &mut eg,
            name,
            Schema {
                input,
                output: "Node".into(),
            },
            None,
            // Nothing may be picked from the table of a symbol without a kernel.
            !has_kernel(symbol),
        )?;
    }
    add_ruleset(&mut eg, "equal")?;
    for (from, to) in rules {
        // Where every node of the one side is found, every node of the
        // other is made, and the two sides are the same.
        let found = from
            .parts
            .iter()
            .map(|(name, made)| Fact::Eq(span!(), exprs::var(name), made.clone()))
            .collect();
        let mut then: Vec<Action> = to
            .parts
            .iter()
            .map(|(name, made)| Action::Let(span!(), name.clone(), made.clone()))
            .collect();
        then.push(Action::Union(span!(), from.whole.clone(), to.whole.clone()));
        rule(&mut eg, "equal", Facts(found), GenericActions(then))?;
    }

    // The graph itself, each step after its inputs.
    let name = |step: usize| format!("$n{step}");
    for (i, step) in graph.steps.iter().enumerate() {
        let mut columns = vec![exprs::string(&step.attrs)];
        columns.extend(step.inputs.iter().map(|x| exprs::var(&name(*x))));
        let made = exprs::call(&table(&step.symbol, step.inputs.len()), columns);
        eg.run_program(vec![Command::Action(Action::Let(span!(), name(i), made))])?;
    }

    // Every form it is equal to.
    let once = Schedule::Run(
        span!(),
        RunConfig {
            ruleset: "equal".into(),
            until: None,
        },
    );
    eg.run_program(vec![Command::RunSchedule(Schedule::Saturate(
        span!(),
        Box::new(once),
    ))])?;

    // One form for each node asked for. What they share is picked the same
    // way for each, so it is in the mapped graph once.
    let mut roots = Vec::new();
    let mut sorts = Vec::new();
    for result in &graph.results {
        let (sort, value) = eg.eval_expr(&exprs::var(&name(*result)))?;
        sorts.push(sort);
        roots.push(value);
    }
    sorts.truncate(1);
    let costs = Extractor::<BigInt>::compute_costs_from_rootsorts(Some(sorts), &eg, Fewest);
    let mut dag = TermDag::default();
    let picked: Option<Vec<TermId>> = roots
        .iter()
        .map(|root| costs.extract_best(&eg, &mut dag, *root).map(|(_, best)| best))
        .collect();
    let Some(picked) = picked else {
        let without: BTreeSet<String> = graph
            .steps
            .iter()
            .filter(|step| !has_kernel(&step.symbol))
            .map(|step| format!("`{}`", step.symbol))
            .collect();
        let without: Vec<String> = without.into_iter().collect();
        let given: Vec<String> = dialects.iter().map(|d| format!("`{}`", d.name)).collect();
        return Err(format!(
            "this graph cannot be mapped onto the dialects given ({}): no form of it has a kernel \
             for every symbol. The symbols in it without one: {}. Each needs a kernel, or an \
             equivalence to another dialect's symbols that have one",
            given.join(", "),
            without.join(", ")
        )
        .into());
    };

    // The picked form as steps, each once. Terms are numbered after what
    // they are made of, so going through them in order is enough.
    let mut wanted = BTreeSet::new();
    let mut waiting: Vec<TermId> = picked.clone();
    while let Some(term) = waiting.pop() {
        if !wanted.insert(term) {
            continue;
        }
        if let Term::App(_, columns) = dag.get(term) {
            waiting.extend(columns[1..].iter().copied());
        }
    }
    let mut mapped = Graph {
        steps: vec![],
        results: vec![],
    };
    let mut at: HashMap<TermId, usize> = HashMap::new();
    for term in wanted {
        let Term::App(table, columns) = dag.get(term) else {
            return Err("what egglog picked is not a symbol applied to nodes".into());
        };
        let Term::Lit(Literal::String(attrs)) = dag.get(columns[0]) else {
            return Err("what egglog picked has attributes that are not text".into());
        };
        let inputs: Option<Vec<usize>> = columns[1..].iter().map(|x| at.get(x).copied()).collect();
        let (Some(inputs), Some((symbol, _))) = (inputs, tables.get(table)) else {
            return Err("what egglog picked is not made of the symbols it was given".into());
        };
        mapped.steps.push(Step {
            symbol: symbol.clone(),
            attrs: attrs.clone(),
            inputs,
        });
        at.insert(term, mapped.steps.len() - 1);
    }
    mapped.results = picked.iter().map(|term| at[term]).collect();
    Ok(mapped)
}
