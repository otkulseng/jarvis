//! The std dialect: the operations we ship, with kernels on every backend.

use ::std::ops::{Add, Div, Index, Mul, Neg, Sub};

use ::cubecl::prelude::*;

use crate::cubecl::{arg, empty, grid, Buf};
use crate::stablehlo::Site;
use crate::{apply, cpu, stablehlo, Array, Op, Value};

#[derive(Clone, Debug)]
pub enum Std {
    Exp,
    Neg,
    Sqrt,
    Add,
    Sub,
    Mul,
    Div,
    Max,
    /// The input's shape, filled with the value.
    Full(f64),
    /// The same numbers in another shape.
    Reshape(Vec<usize>),
    /// The input repeated along its axes of size 1, to a shape of the same rank.
    Broadcast(Vec<usize>),
    /// Sum over the axis.
    Sum(usize),
    /// Rows of the first input at the i32 indices of the second.
    Take,
    /// `out[idx[e]] += src[e]` over the rows of `src`, into this many rows.
    ScatterAdd(usize),
    /// `[n, k] @ [k, m]`. Means a broadcast product summed over k.
    Matmul,
}
use Std::*;

impl Op for Std {
    fn dialect(&self) -> &'static str {
        "std"
    }

    fn shape(&self, s: &[&[usize]]) -> Vec<usize> {
        match self {
            Sum(axis) => [&s[0][..*axis], &s[0][*axis + 1..]].concat(),
            Take => [&s[1][..1], &s[0][1..]].concat(),
            ScatterAdd(n) => [&[*n][..], &s[0][1..]].concat(),
            Reshape(t) | Broadcast(t) => t.clone(),
            Matmul => vec![s[0][0], s[1][1]],
            _ => s[0].to_vec(),
        }
    }

    fn equivalent(&self, i: &[Array]) -> Option<Array> {
        match self {
            Matmul => Some((i[0].expand_dims(2) * i[1].expand_dims(0)).sum(1)),
            _ => None,
        }
    }

    fn cpu(&self) -> Option<cpu::Kernel<'_>> {
        let op = self.clone();
        Some(Box::new(move |i| match &op {
            Exp => map(i[0], f32::exp),
            Neg => map(i[0], |v| -v),
            Sqrt => map(i[0], f32::sqrt),
            Add => zip(i[0], i[1], |a, b| a + b),
            Sub => zip(i[0], i[1], |a, b| a - b),
            Mul => zip(i[0], i[1], |a, b| a * b),
            Div => zip(i[0], i[1], |a, b| a / b),
            Max => zip(i[0], i[1], f32::max),
            Full(v) => Value::new(&i[0].shape, vec![*v as f32; i[0].f32().len()]),
            Reshape(t) => Value::new(t, i[0].f32().to_vec()),
            Broadcast(t) => {
                let (x, st) = (i[0].f32(), strides(&i[0].shape));
                let n: usize = t.iter().product();
                let at = |o: usize| (0..t.len()).rev().fold((o, 0), |(rem, off), d| (rem / t[d], off + rem % t[d] * st[d])).1;
                Value::new(t, (0..n).map(|o| x[at(o)]).collect())
            }
            Sum(axis) => {
                let (outer, len, inner) = split(&i[0].shape, *axis);
                let x = i[0].f32();
                let mut out = vec![0.0; outer * inner];
                for o in 0..outer {
                    for k in 0..len {
                        for c in 0..inner {
                            out[o * inner + c] += x[(o * len + k) * inner + c];
                        }
                    }
                }
                Value::new(&op.shape(&[&i[0].shape]), out)
            }
            Take => {
                let row: usize = i[0].shape[1..].iter().product();
                let x = i[0].f32();
                let out: Vec<f32> = i[1].i32().iter().flat_map(|j| &x[*j as usize * row..(*j as usize + 1) * row]).copied().collect();
                Value::new(&op.shape(&[&i[0].shape, &i[1].shape]), out)
            }
            ScatterAdd(n) => {
                let row: usize = i[0].shape[1..].iter().product();
                let mut out = vec![0.0; n * row];
                for (e, j) in i[1].i32().iter().enumerate() {
                    for c in 0..row {
                        out[*j as usize * row + c] += i[0].f32()[e * row + c];
                    }
                }
                Value::new(&op.shape(&[&i[0].shape]), out)
            }
            Matmul => {
                let (a, b) = (i[0].f32(), i[1].f32());
                let (n, k, m) = (i[0].shape[0], i[0].shape[1], i[1].shape[1]);
                let out: Vec<f32> = (0..n * m).map(|o| (0..k).map(|x| a[o / m * k + x] * b[x * m + o % m]).sum()).collect();
                Value::new(&[n, m], out)
            }
        }))
    }

    fn stablehlo(&self) -> Option<stablehlo::Kernel<'_>> {
        let op = self.clone();
        Some(Box::new(move |s| match &op {
            Exp => unary(s, "exponential"),
            Neg => unary(s, "negate"),
            Sqrt => unary(s, "sqrt"),
            Add => binary_hlo(s, "add"),
            Sub => binary_hlo(s, "subtract"),
            Mul => binary_hlo(s, "multiply"),
            Div => binary_hlo(s, "divide"),
            Max => binary_hlo(s, "maximum"),
            Full(v) => format!("  {} = stablehlo.constant dense<{v:?}> : {}\n", s.out, s.out_ty),
            Reshape(_) => format!("  {} = stablehlo.reshape {} : ({}) -> {}\n", s.out, s.ins[0], s.tys[0], s.out_ty),
            Broadcast(t) => format!(
                "  {} = stablehlo.broadcast_in_dim {}, dims = [{}] : ({}) -> {}\n",
                s.out, s.ins[0], list(0..t.len()), s.tys[0], s.out_ty
            ),
            Sum(axis) => format!(
                "  {o}_z = stablehlo.constant dense<0.0> : tensor<f32>\n  {o} = stablehlo.reduce({x} init: {o}_z) applies stablehlo.add across dimensions = [{axis}] : ({ty}, tensor<f32>) -> {oty}\n",
                o = s.out, x = s.ins[0], ty = s.tys[0], oty = s.out_ty
            ),
            Take => format!(
                "  {o} = \"stablehlo.gather\"({x}, {i}) {{ dimension_numbers = #stablehlo.gather<offset_dims = [{offset}], collapsed_slice_dims = [0], start_index_map = [0], index_vector_dim = 1>, slice_sizes = array<i64: 1{sizes}>, indices_are_sorted = false }} : ({tx}, {ti}) -> {oty}\n",
                o = s.out, x = s.ins[0], i = s.ins[1], offset = list(1..s.shapes[0].len()),
                sizes = s.shapes[0][1..].iter().map(|d| format!(", {d}")).collect::<String>(), tx = s.tys[0], ti = s.tys[1], oty = s.out_ty
            ),
            ScatterAdd(_) => format!(
                "  {o}_z = stablehlo.constant dense<0.0> : {oty}\n  {o} = \"stablehlo.scatter\"({o}_z, {i}, {x}) ({{\n  ^bb0(%a: tensor<f32>, %b: tensor<f32>):\n    %s = stablehlo.add %a, %b : tensor<f32>\n    stablehlo.return %s : tensor<f32>\n  }}) {{ scatter_dimension_numbers = #stablehlo.scatter<update_window_dims = [{window}], inserted_window_dims = [0], scatter_dims_to_operand_dims = [0], index_vector_dim = 1>, indices_are_sorted = false, unique_indices = false }} : ({oty}, {ti}, {tx}) -> {oty}\n",
                o = s.out, x = s.ins[0], i = s.ins[1], window = list(1..s.shapes[0].len()), tx = s.tys[0], ti = s.tys[1], oty = s.out_ty
            ),
            Matmul => format!(
                "  {} = stablehlo.dot_general {}, {}, contracting_dims = [1] x [0] : ({}, {}) -> {}\n",
                s.out, s.ins[0], s.ins[1], s.tys[0], s.tys[1], s.out_ty
            ),
        }))
    }

    fn cubecl(&self) -> Option<crate::cubecl::Kernel<'_>> {
        if let Matmul = self {
            return None;
        }
        let op = self.clone();
        Some(Box::new(move |c, i, shape| {
            if let Reshape(_) = op {
                return Buf { handle: i[0].handle.clone(), shape: shape.to_vec() };
            }
            let out = empty(c, shape);
            let (count, dim) = grid(out.len());
            unsafe {
                match &op {
                    Exp => k_exp::launch_unchecked(c, count, dim, arg(i[0]), arg(&out)),
                    Neg => k_neg::launch_unchecked(c, count, dim, arg(i[0]), arg(&out)),
                    Sqrt => k_sqrt::launch_unchecked(c, count, dim, arg(i[0]), arg(&out)),
                    Add => k_add::launch_unchecked(c, count, dim, arg(i[0]), arg(i[1]), arg(&out)),
                    Sub => k_sub::launch_unchecked(c, count, dim, arg(i[0]), arg(i[1]), arg(&out)),
                    Mul => k_mul::launch_unchecked(c, count, dim, arg(i[0]), arg(i[1]), arg(&out)),
                    Div => k_div::launch_unchecked(c, count, dim, arg(i[0]), arg(i[1]), arg(&out)),
                    Max => k_max::launch_unchecked(c, count, dim, arg(i[0]), arg(i[1]), arg(&out)),
                    Full(v) => k_full::launch_unchecked(c, count, dim, arg(&out), *v as f32),
                    Broadcast(t) => {
                        // one repeat per axis of length 1, from the last axis up
                        let mut cur = Buf { handle: i[0].handle.clone(), shape: i[0].shape.clone() };
                        for d in (0..t.len()).rev() {
                            if cur.shape[d] == t[d] {
                                continue;
                            }
                            let mut shape = cur.shape.clone();
                            shape[d] = t[d];
                            let next = empty(c, &shape);
                            let inner: usize = shape[d + 1..].iter().product();
                            let (count, dim) = grid(next.len());
                            k_repeat::launch_unchecked(c, count, dim, arg(&cur), arg(&next), (t[d] * inner) as u32, inner as u32);
                            cur = next;
                        }
                        return cur;
                    }
                    Sum(axis) => {
                        // `shape` is the input's without the axis
                        let inner: usize = shape[*axis..].iter().product();
                        let len = i[0].shape[*axis];
                        k_sum::launch_unchecked(c, count, dim, arg(i[0]), arg(&out), len as u32, inner as u32)
                    }
                    Take => {
                        let row: usize = shape[1..].iter().product();
                        k_take::launch_unchecked(c, count, dim, arg(i[0]), arg(i[1]), arg(&out), row as u32)
                    }
                    ScatterAdd(_) => {
                        let row: usize = shape[1..].iter().product();
                        k_scatter_add::launch_unchecked(c, count, dim, arg(i[0]), arg(i[1]), arg(&out), row as u32)
                    }
                    Reshape(_) | Matmul => unreachable!(),
                }
            }
            out
        }))
    }
}

// The frontend.

/// The right side of a binary operation: an array, or a number that takes the left side's shape.
pub trait Operand {
    fn with(self, like: Array) -> Array;
}
impl Operand for Array {
    fn with(self, _: Array) -> Array {
        self
    }
}
impl Operand for f64 {
    fn with(self, like: Array) -> Array {
        like.full(self)
    }
}

impl Array {
    pub fn exp(self) -> Array {
        apply(Exp, &[self])
    }
    pub fn sqrt(self) -> Array {
        apply(Sqrt, &[self])
    }
    pub fn full(self, value: f64) -> Array {
        apply(Full(value), &[self])
    }
    pub fn maximum(self, rhs: impl Operand) -> Array {
        binary(Max, self, rhs)
    }
    pub fn reshape(self, shape: &[usize]) -> Array {
        apply(Reshape(shape.to_vec()), &[self])
    }
    /// A new axis of length 1 at `axis`: numpy's `x[:, None]`.
    pub fn expand_dims(self, axis: usize) -> Array {
        let mut s = self.shape();
        s.insert(axis, 1);
        self.reshape(&s)
    }
    pub fn sum(self, axis: usize) -> Array {
        apply(Sum(axis), &[self])
    }
    /// Rows of `self` at `idx`. Also written `self[idx]`.
    pub fn take(self, idx: Array) -> Array {
        apply(Take, &[self, idx])
    }
    pub fn scatter_add(self, idx: Array, n: usize) -> Array {
        apply(ScatterAdd(n), &[self, idx])
    }
    pub fn matmul(self, rhs: Array) -> Array {
        apply(Matmul, &[self, rhs])
    }
}

/// A binary operation with numpy's broadcasting: ranks are aligned on the
/// right, and an axis of length 1 is repeated to match the other side.
fn binary(op: Std, a: Array, rhs: impl Operand) -> Array {
    let b = rhs.with(a);
    let (sa, sb) = (a.shape(), b.shape());
    let rank = sa.len().max(sb.len());
    let pad = |s: &[usize]| [&vec![1; rank - s.len()][..], s].concat();
    let (pa, pb) = (pad(&sa), pad(&sb));
    let out: Vec<usize> = pa.iter().zip(&pb).map(|(x, y)| {
        assert!(x == y || *x == 1 || *y == 1, "cannot broadcast {sa:?} with {sb:?}");
        *x.max(y)
    }).collect();
    let fit = |x: Array, s: &[usize], p: &[usize]| {
        let x = if s == p { x } else { x.reshape(p) };
        if p == out { x } else { apply(Broadcast(out.clone()), &[x]) }
    };
    apply(op, &[fit(a, &sa, &pa), fit(b, &sb, &pb)])
}

impl Neg for Array {
    type Output = Array;
    fn neg(self) -> Array {
        apply(Std::Neg, &[self])
    }
}

/// `x[idx]` is `x.take(idx)`. `Index` must hand out a reference, so the handle
/// it returns is leaked: eight bytes, as long as the node they name lives anyway.
impl Index<Array> for Array {
    type Output = Array;
    fn index(&self, idx: Array) -> &Array {
        Box::leak(Box::new(self.take(idx)))
    }
}

/// `a + b`, `a + 2.0` and `2.0 + a`.
macro_rules! binop {
    ($tr:ident, $f:ident, $op:expr) => {
        impl<R: Operand> $tr<R> for Array {
            type Output = Array;
            fn $f(self, rhs: R) -> Array {
                binary($op, self, rhs)
            }
        }
        impl $tr<Array> for f64 {
            type Output = Array;
            fn $f(self, rhs: Array) -> Array {
                binary($op, rhs.full(self), rhs)
            }
        }
    };
}
binop!(Add, add, Std::Add);
binop!(Sub, sub, Std::Sub);
binop!(Mul, mul, Std::Mul);
binop!(Div, div, Std::Div);

// Helpers for the kernels.

fn map(x: &Value, f: fn(f32) -> f32) -> Value {
    Value::new(&x.shape, x.f32().iter().map(|v| f(*v)).collect())
}
fn zip(x: &Value, y: &Value, f: fn(f32, f32) -> f32) -> Value {
    assert_eq!(x.shape, y.shape);
    Value::new(&x.shape, x.f32().iter().zip(y.f32()).map(|(a, b)| f(*a, *b)).collect())
}
/// Splits a shape at `axis` into (outer, len, inner).
fn split(shape: &[usize], axis: usize) -> (usize, usize, usize) {
    (shape[..axis].iter().product(), shape[axis], shape[axis + 1..].iter().product())
}
/// Row-major strides, 0 along an axis of length 1 so that it repeats.
fn strides(shape: &[usize]) -> Vec<usize> {
    let mut s = vec![0; shape.len()];
    let mut acc = 1;
    for d in (0..shape.len()).rev() {
        s[d] = if shape[d] == 1 { 0 } else { acc };
        acc *= shape[d];
    }
    s
}
fn list(r: ::std::ops::Range<usize>) -> String {
    r.map(|d| d.to_string()).collect::<Vec<_>>().join(", ")
}
fn unary(s: &Site, op: &str) -> String {
    format!("  {} = stablehlo.{op} {} : {}\n", s.out, s.ins[0], s.out_ty)
}
fn binary_hlo(s: &Site, op: &str) -> String {
    format!("  {} = stablehlo.{op} {}, {} : {}\n", s.out, s.ins[0], s.ins[1], s.out_ty)
}

// CubeCL kernels.

#[cube(launch_unchecked)]
fn k_exp(x: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = f32::exp(x[ABSOLUTE_POS]);
    }
}
#[cube(launch_unchecked)]
fn k_neg(x: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = -x[ABSOLUTE_POS];
    }
}
#[cube(launch_unchecked)]
fn k_sqrt(x: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = f32::sqrt(x[ABSOLUTE_POS]);
    }
}
#[cube(launch_unchecked)]
fn k_add(x: &[f32], z: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = x[ABSOLUTE_POS] + z[ABSOLUTE_POS];
    }
}
#[cube(launch_unchecked)]
fn k_sub(x: &[f32], z: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = x[ABSOLUTE_POS] - z[ABSOLUTE_POS];
    }
}
#[cube(launch_unchecked)]
fn k_mul(x: &[f32], z: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = x[ABSOLUTE_POS] * z[ABSOLUTE_POS];
    }
}
#[cube(launch_unchecked)]
fn k_div(x: &[f32], z: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = x[ABSOLUTE_POS] / z[ABSOLUTE_POS];
    }
}
#[cube(launch_unchecked)]
fn k_max(x: &[f32], z: &[f32], y: &mut [f32]) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = f32::max(x[ABSOLUTE_POS], z[ABSOLUTE_POS]);
    }
}
#[cube(launch_unchecked)]
fn k_full(y: &mut [f32], v: f32) {
    if ABSOLUTE_POS < y.len() {
        y[ABSOLUTE_POS] = v;
    }
}
#[cube(launch_unchecked)]
fn k_repeat(x: &[f32], y: &mut [f32], block: u32, inner: u32) {
    if ABSOLUTE_POS < y.len() {
        let o = ABSOLUTE_POS / block as usize;
        let c = ABSOLUTE_POS % inner as usize;
        y[ABSOLUTE_POS] = x[o * inner as usize + c];
    }
}
#[cube(launch_unchecked)]
fn k_sum(x: &[f32], y: &mut [f32], len: u32, inner: u32) {
    if ABSOLUTE_POS < y.len() {
        let o = ABSOLUTE_POS / inner as usize;
        let c = ABSOLUTE_POS % inner as usize;
        let mut acc = 0.0f32;
        for k in 0..len as usize {
            acc += x[(o * len as usize + k) * inner as usize + c];
        }
        y[ABSOLUTE_POS] = acc;
    }
}
#[cube(launch_unchecked)]
fn k_take(x: &[f32], idx: &[i32], y: &mut [f32], row: u32) {
    if ABSOLUTE_POS < y.len() {
        let e = ABSOLUTE_POS / row as usize;
        let c = ABSOLUTE_POS % row as usize;
        y[ABSOLUTE_POS] = x[idx[e] as usize * row as usize + c];
    }
}
#[cube(launch_unchecked)]
fn k_scatter_add(src: &[f32], idx: &[i32], y: &mut [f32], row: u32) {
    if ABSOLUTE_POS < y.len() {
        let n = ABSOLUTE_POS / row as usize;
        let c = ABSOLUTE_POS % row as usize;
        let mut acc = 0.0f32;
        for e in 0..idx.len() {
            if idx[e] as usize == n {
                acc += src[e * row as usize + c];
            }
        }
        y[ABSOLUTE_POS] = acc;
    }
}
