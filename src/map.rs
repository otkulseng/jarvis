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
use crate::{applied, dialect_of, Dialect, Graph, Node, Step, ANY};
use egglog::ast::*;
use egglog::extract::{CostModel, Extractor};
use egglog::prelude::*;
use egglog::{ArcSort, EGraph, Enode, Function, Term, TermDag, TermId};
use num::BigInt;
use std::collections::{BTreeMap, BTreeSet, HashMap};

fn table(symbol: &str, inputs: usize) -> String {
    format!("{symbol}/{inputs}")
}

/// One side of an equivalence as egglog reads it in a rule: `ANY` nodes are
/// variables. The tables it uses and its variables are noted on the way.
fn pattern(
    node: Node,
    tables: &mut BTreeMap<String, (String, usize)>,
    variables: &mut BTreeSet<String>,
) -> Expr {
    let made = applied(node);
    if made.symbol == ANY {
        let name = format!("any{}", made.attrs);
        variables.insert(name.clone());
        return exprs::var(&name);
    }
    let name = table(&made.symbol, made.inputs.len());
    tables.insert(name.clone(), (made.symbol.clone(), made.inputs.len()));
    let mut columns = vec![exprs::string(&made.attrs)];
    columns.extend(made.inputs.iter().map(|x| pattern(*x, tables, variables)));
    exprs::call(&name, columns)
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
        let (mut left_has, mut right_has) = (BTreeSet::new(), BTreeSet::new());
        let left_reads = pattern(*left, &mut tables, &mut left_has);
        let right_reads = pattern(*right, &mut tables, &mut right_has);
        if applied(*left).symbol != ANY && right_has.is_subset(&left_has) {
            rules.push((left_reads.clone(), right_reads.clone()));
        }
        if applied(*right).symbol != ANY && left_has.is_subset(&right_has) {
            rules.push((right_reads, left_reads));
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
        rule(
            &mut eg,
            "equal",
            Facts(vec![Fact::Eq(span!(), exprs::var("it"), from)]),
            GenericActions(vec![Action::Union(span!(), exprs::var("it"), to)]),
        )?;
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
