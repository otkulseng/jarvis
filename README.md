# n-ary

`nary` is a small, NumPy-like frontend for scientific computing in Rust. Write the science clearly, in the operations of your field, and have it run on the fastest code anyone has written for it.

**nary is not a compiler.** It does not optimize, lower, fuse, schedule, or generate code. Every piece of code that computes something is a *kernel*, written by a person or an agent. nary does one thing: it maps the graph a program builds onto the kernels that exist.

The main bet is that this is enough when agents can write kernels. An agent can replace any region of a computation with a specialized kernel, up to the entire graph between two breaks. The scientific code stays the same.

## The pieces

- A **symbol** belongs to a dialect and is named `dialect.name`: `np.add`, `md.lj_forces`.
- A **node** is a symbol applied to nodes. That is all nary knows about it. nary does not know what a node stands for, what its value is, or how it is computed.
- A **dialect** is a value that holds two things, both optional:
  - **equivalences**: an expression in its symbols is the same as an expression in another dialect's symbols;
  - **kernels**: code that computes one of its symbols from the results of its inputs.
- To **map** a graph is to find a form of it in which every symbol has a kernel.
- To **run** a mapped graph is to call those kernels, each after its inputs.

No dialect is special. A symbol can be an addition, a neighbour calculation, or a whole simulation step. A dialect of arrays (`np/`) and a dialect with one symbol for an entire time step are the same kind of thing.

```rust
pub fn euler_step(pos: Array, mass: Array, dt: Array) -> Array {
    nary::symbol("toy.euler_step", [pos, mass, dt])
}

pub fn dialect() -> nary::Dialect {
    nary::Dialect::new("toy")
        // What the symbol means, in np.
        .equal(|[pos, mass, dt]| (
            euler_step(pos, mass, dt),
            np::add(pos, np::mul(dt, np::div(np::neg(pos), mass))),
        ))
        // Code that computes the whole step at once.
        .kernel("toy.euler_step", |x: &[&np::Value]| { /* one pass over the particles */ })
}
```

## Mapping, and what the e-graph is for

An equivalence is read in both directions. It says what a symbol means in another dialect, and it lets that meaning be recognized inside a graph. So a program written entirely in `np` is mapped onto `toy.euler_step` when the `toy` dialect is given, without ever naming it. This is how a kernel written once is used by every later program that computes the same thing.

The graph and the equivalences are put into an e-graph ([egglog](https://github.com/egraphs-good/egglog)), which holds every form the graph is equal to. One form is then picked in which every symbol has a kernel.

That is the only use of the e-graph: **mapping subgraphs onto kernels**. It is not used to simplify or optimize. An equivalence must be between two dialects; what is equal within one dialect is for the code of that dialect to use.

The e-graph is there for two reasons:

1. **It shows that a valid mapping exists.** Either some form of the graph has a kernel for every symbol, or mapping fails and names the symbols that have none.
2. **Derivatives** (planned). The derivative through a node can be taken through the symbol itself, if it has one, or through anything the node is equal to. The e-graph already holds those alternatives.

Which form is picked is, for now, simply the shortest: the one with the fewest symbols. Larger kernels are therefore preferred over many small ones. Estimating latencies instead is a possible future. It is not the point today.

Recognition goes exactly as far as the declared equivalences. nary does not know that addition commutes unless a dialect's own code arranges for it.

## Rust owns the graph breaks

Operations on arrays build a graph and compute nothing. Calling `get` maps the graph of the requested node, runs its kernels, and returns the value. `get_all` does the same for several nodes at once, as one graph: what they share is computed once.

A value that has been computed is ordinary Rust data. Rust handles the branch that depends on it, errors, I/O, checkpointing, and the surrounding application. The graph is complete up to that boundary.

## How performance is obtained

Since nary does not optimize, speed comes from kernels:

1. Write the computation in a general dialect such as `np`, and run it on that dialect's kernels.
2. Profile. Find a region that is expensive.
3. Give the region a name: a symbol in a dialect, with an equivalence that states what it means.
4. Have an agent write a kernel for it, for the hardware at hand. The kernel may change the algorithm, call a library, or do the whole region in one pass. What happens inside a kernel is its own business.
5. Check the kernel against the meaning: both ways of computing the result must agree.
6. Pass the dialect to `get`. The program does not change.

A symbol with a meaning and no kernel still runs, through what it equals. A symbol with a kernel and no meaning is a primitive of its own.

## Bets

- A clear default plus a principled way to add specialized kernels is a useful foundation for scientific computing.
- Agents can find bottlenecks and write correct, competitive kernels from profiling feedback.
- Large kernels can absorb what no general optimizer would get right, so nary does not need one.
- Scientific computations recur across programs and dialects often enough that kernels written once are found again.

## Current state

The code is a small prototype of exactly the above.

```text
src/        nary: symbols, nodes, dialects, map, run, get
np/         a dialect of arrays: arithmetic with NumPy's rules for shapes, sums along
            an axis, comparison, constants, random numbers, values handed over by Rust
md/         a dialect of molecular dynamics: Lennard-Jones forces and energy in a
            periodic box
examples/   argon.rs: liquid argon, after Rahman (1964)
            euler.rs: the smallest whole example, with a dialect of its own: a hundred
            steps written in np, mapped onto ten kernels
tests/      making a dialect, the arrays of np, md's kernels against their meaning
```

```sh
cargo test --workspace
cargo run --release --example argon
cargo run --example euler
```

Kernels run on the CPU, one after the other, on values the dialects define themselves. Derivatives are not implemented: a force is written by hand, as a symbol of its own. Neither is any choice between kernels other than the shortest graph.

Every `get` maps its graph from the beginning, and that takes a fixed time of some tens of milliseconds with the dialects here, whatever the size of the graph. Nothing is kept from one `get` to the next. Until something is, a program should compute many steps in one `get`.

### Liquid argon

`examples/argon.rs` is 864 Lennard-Jones atoms at 94.4 K and 1.374 g/cm³. The time step is written in np, and the forces are `md.lj_forces`. That symbol means a sum over all pairs of atoms, in np. Its kernel looks only at atoms in neighbouring cells of the box: a different algorithm, which the equivalence lets the tests check against the meaning.

Every hundred steps the program gets the atoms back, and Rust measures the temperature, rescales the velocities while the liquid settles, and counts distances for g(r). The first peak of g(r) comes out at 3.7 Å with a height of 2.8, to be held against Rahman's figure 2.

With `-- 3 np` the same program runs 108 atoms on np's kernels alone.
