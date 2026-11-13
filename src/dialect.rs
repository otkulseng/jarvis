//! A dialect: what its symbols equal in other dialects, and the code that
//! computes them.

use crate::{applied, dialect_of, make, Applied, Node, ANY};
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;

/// The code of a kernel, as nary keeps it: the symbol's attributes and the
/// results of its inputs go in, its result comes out.
pub(crate) type Code = Rc<dyn Fn(&str, &[&dyn Any]) -> Result<Box<dyn Any>, String>>;

/// A dialect, as a value: build one, print it, pass it to `map` and `run`.
///
/// ```ignore
/// nary::Dialect::new("toy")
///     .equal(|[pos, mass, dt]| (
///         euler_step(pos, mass, dt),
///         np::add(pos, np::mul(dt, np::div(np::neg(pos), mass))),
///     ))
///     .kernel("toy.euler_step", |x: &[&np::Value]| { /* the whole step */ })
/// ```
#[derive(Clone)]
pub struct Dialect {
    pub(crate) name: String,
    /// The two sides of each equivalence, over `ANY` nodes.
    pub(crate) equal: Vec<(Node, Node)>,
    pub(crate) kernels: BTreeMap<String, Code>,
    /// What was wrong in how it was built. `map` and `run` return these.
    pub(crate) problems: Vec<String>,
}

impl Dialect {
    pub fn new(name: &str) -> Dialect {
        Dialect {
            name: name.into(),
            equal: vec![],
            kernels: BTreeMap::new(),
            problems: vec![],
        }
    }

    /// The two expressions are the same, whatever they are given. They must
    /// not both be written in one dialect alone: what is the same within a
    /// dialect is for the code that builds its graphs to use.
    pub fn equal<T, const N: usize>(mut self, sides: impl FnOnce([T; N]) -> (T, T)) -> Dialect
    where
        T: From<Node> + Into<Node>,
    {
        let any: [T; N] = std::array::from_fn(|i| {
            make(Applied {
                symbol: ANY.into(),
                attrs: i.to_string(),
                inputs: vec![],
            })
            .into()
        });
        let (left, right) = sides(any);
        let (left, right) = (left.into(), right.into());
        let mut dialects = dialects_in(left);
        dialects.extend(dialects_in(right));
        if dialects.len() < 2 {
            self.problems.push(format!(
                "an equivalence is between dialects, and both sides of this one are written in \
                 `{}` alone: {} = {}",
                dialects.into_iter().next().unwrap_or_default(),
                written(left),
                written(right)
            ));
            return self;
        }
        self.equal.push((left, right));
        self
    }

    /// This code computes the symbol from the results of its inputs. All of
    /// them, and its own result, are of one type, of the kernel's choosing.
    pub fn kernel<V: 'static>(self, symbol: &str, code: impl Fn(&[&V]) -> V + 'static) -> Dialect {
        self.kernel_with(symbol, move |_, inputs| code(inputs))
    }

    /// A kernel for a symbol that is given attributes: they come first.
    pub fn kernel_with<V: 'static>(
        mut self,
        symbol: &str,
        code: impl Fn(&str, &[&V]) -> V + 'static,
    ) -> Dialect {
        if dialect_of(symbol) != Some(self.name.as_str()) {
            self.problems.push(format!(
                "the dialect `{}` cannot hold a kernel for `{symbol}`: its own symbols are named `{}.…`",
                self.name, self.name
            ));
            return self;
        }
        let name = symbol.to_string();
        let erased: Code = Rc::new(move |attrs, inputs| {
            let mut typed: Vec<&V> = Vec::with_capacity(inputs.len());
            for (i, input) in inputs.iter().enumerate() {
                typed.push(input.downcast_ref::<V>().ok_or_else(|| {
                    format!(
                        "the kernel of `{name}` computes on `{}`, and its input {i} is something else",
                        std::any::type_name::<V>()
                    )
                })?);
            }
            Ok(Box::new(code(attrs, &typed)))
        });
        self.kernels.insert(symbol.into(), erased);
        self
    }
}

/// A dialect is printed as what it holds.
impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "dialect {}", self.name)?;
        for (left, right) in &self.equal {
            writeln!(f, "  equal   {} = {}", written(*left), written(*right))?;
        }
        for symbol in self.kernels.keys() {
            writeln!(f, "  kernel  {symbol}")?;
        }
        for problem in &self.problems {
            writeln!(f, "  WRONG   {problem}")?;
        }
        Ok(())
    }
}

/// What is wrong with any of the dialects, as the error of whatever was
/// asked of them.
pub(crate) fn sound(dialects: &[Dialect]) -> Result<(), String> {
    let problems: Vec<String> = dialects
        .iter()
        .flat_map(|d| {
            d.problems
                .iter()
                .map(move |p| format!("in the dialect `{}`: {p}", d.name))
        })
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

/// An expression on one line, with `?0`, `?1` for "any node".
pub(crate) fn written(node: Node) -> String {
    let made = applied(node);
    if made.symbol == ANY {
        return format!("?{}", made.attrs);
    }
    let inputs: Vec<String> = made.inputs.iter().map(|x| written(*x)).collect();
    let attrs = if made.attrs.is_empty() {
        String::new()
    } else {
        format!("[{}]", made.attrs)
    };
    format!("{}{attrs}({})", made.symbol, inputs.join(", "))
}

/// The dialects whose symbols an expression is written in.
fn dialects_in(node: Node) -> BTreeSet<String> {
    let made = applied(node);
    let mut all = BTreeSet::new();
    if made.symbol != ANY {
        all.insert(dialect_of(&made.symbol).unwrap_or(&made.symbol).to_string());
    }
    for input in made.inputs {
        all.extend(dialects_in(input));
    }
    all
}
