# n-ary

`nary` is a small, NumPy-like frontend for scientific computing in Rust. Write the science clearly and get a fairly fast implementation by default. The default may be slower than JAX or PyTorch.

The main bet is that this is useful when there is a principled agentic route to optimize as far as the workload warrants. Agents can replace any computational region with a specialized implementation, up to the entire graph between breaks. The scientific code stays the same.

The architecture should impose no fixed menu of optimizations. Definitions establish what an implementation must compute. Measurements establish whether it is worth using. The registry keeps that work available for later computations.

## The optimization loop

1. Run the default implementation and profile the workload.
2. Identify an expensive region and express its meaning as a dialect operation.
3. Have agents implement it for the target hardware.
4. Check its numerical contract, benchmark it, and fit its cost model.
5. Register it and measure the resulting execution plan.
6. Keep useful implementations and repeat wherever further work is worthwhile.

An agent can change the algorithm, fuse operations, call a library, or write an entire execution schedule. The operation boundary allows all of these choices.

## Operations and dialects

Every operation belongs to a dialect. An operation can describe an addition, a neighbour calculation, or an entire simulation step. Its size is unrestricted.

An operation declares its inputs, outputs, attributes, and constraints. It can define its meaning through operations in other dialects. These definitions connect dialects through equivalence. There is no required ordering from high-level to low-level dialects.

A kernel implements a dialect operation. Here, a kernel means an executable implementation. It may contain several GPU launches, Rust code, library calls, temporary storage, or communication.

Implementations own those decisions. The frontend records the requested computation.

## Making optimization cumulative

Suppose one dialect defines an operation as `Add(Mul(x, y), z)`. An agent writes a fast implementation for it.

Another program expresses that computation using a different dialect. If the declared equivalences connect the two forms, the system can use the existing implementation. The program does not need to name the original dialect.

Definitions must work in both directions. They must expand an operation into its meaning and recognize that meaning inside another graph. Expanding only the operations already present would miss installed kernels.

An e-graph holds the discovered alternatives. Adding a kernel creates another implementation choice. Existing choices remain available.

## Rust owns the graph breaks

Array operations build a graph. Calling `get()` selects implementations, prepares an executable, and runs the graph needed for that result. Compatible later calls can use a cached executable.

An expression such as `graph.get() < 0` returns control to ordinary Rust. Rust handles the branch, errors, I/O, checkpointing, and the surrounding application.

The graph is complete up to that boundary. Shapes may be symbolic during construction. An implementation can specialize when dimensions become known.

## From graph to execution

1. Collect the graph required for the requested outputs.
2. Apply registered equivalences to discover matching operations across dialects.
3. Find implementations whose device and input requirements are satisfied.
4. Select a composition using hardware-dependent cost estimates.
5. Compile or prepare the selected implementations and connect their inputs and outputs.
6. Execute the plan and return the requested values.

Every selected operation must have an implementation. Its internal decomposition does not need separate kernels.

Implementations declare buffer locations, layouts, and completion behavior. Required transfers and conversions belong in the plan and its cost.


## Bets

- A fairly fast default plus a principled route to specialized performance is a useful foundation for scientific computing.
- Agents can find bottlenecks and produce correct, competitive implementations from profiling feedback.
- Cost models can rank compositions well enough to avoid benchmarking every possibility.
- Larger implementations can absorb interactions that smaller implementations or their cost models handle poorly.
- Useful scientific computations recur across programs and dialects often enough for the registry to amortize optimization effort.


## Derivatives

Differentiation is intended to transform graphs. An operation with a differentiable decomposition can obtain its derivative through that decomposition. An agent can later add an implementation of the derivative operation.

Repeated differentiation should remain possible wherever the operation's mathematics supports it.

## Current prototype

The code demonstrates graph construction, declared equivalences, extraction, and execution through Rust CPU code, StableHLO with PJRT, and CubeCL. It does not yet implement the registry described above. `DESIGN.md` describes the current mechanics.

StableHLO provides a compilation route and a reference implementation path. Individual implementations may use other backends.

`examples/main.rs` builds a small potential from neighbour distances, a radial basis, and a neural network.

```sh
pixi run cpu
pixi run stablehlo
pixi run cubecl
pixi run test
```

The StableHLO path needs a PJRT plugin at `.pjrt/libpjrt_cpu.so`. The configured CubeCL environment uses a software Vulkan device.