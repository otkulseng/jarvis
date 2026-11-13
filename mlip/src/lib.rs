//! A dialect a researcher ships: operations, what each means written in std,
//! and kernels where it has them. A backend without a kernel runs the meaning.

use ::cubecl::prelude::*;
use nary::cubecl::{arg, empty, grid};
use nary::{apply, cpu, input, Array, Op, Value};

#[derive(Clone, Copy, Debug)]
pub enum Mlip {
    /// The length of each row: `sqrt(sum(d * d, last axis))`.
    Norm,
    /// A smooth cutoff, zero past `rc`: `max(0, 1 - r / rc)^2`.
    Cutoff { rc: f64 },
    /// `k` gaussians on `[0, rc]` times the cutoff, one column each.
    Basis { k: usize, rc: f64 },
}
use Mlip::*;

pub fn norm(d: Array) -> Array {
    apply(Norm, &[d])
}
pub fn cutoff(r: Array, rc: f64) -> Array {
    apply(Cutoff { rc }, &[r])
}
pub fn basis(r: Array, k: usize, rc: f64) -> Array {
    apply(Basis { k, rc }, &[r])
}

impl Op for Mlip {
    fn dialect(&self) -> &'static str {
        "mlip"
    }

    fn shape(&self, s: &[&[usize]]) -> Vec<usize> {
        match *self {
            Norm => s[0][..s[0].len() - 1].to_vec(),
            Cutoff { .. } => s[0].to_vec(),
            Basis { k, .. } => [&s[0][..], &[k]].concat(),
        }
    }

    fn equivalent(&self, i: &[Array]) -> Option<Array> {
        let r = i[0];
        Some(match *self {
            Norm => (r * r).sum(r.shape().len() - 1).sqrt(),
            Cutoff { rc } => {
                let t = (1.0 - r / rc).maximum(0.0);
                t * t
            }
            Basis { k, rc } => {
                let mu = input(Value::new(&[k], centers(k, rc)));
                let x = (r.expand_dims(1) - mu.expand_dims(0)) * (k as f64 / rc);
                (-(x * x)).exp() * cutoff(r, rc).expand_dims(1)
            }
        })
    }

    fn cpu(&self) -> Option<cpu::Kernel<'_>> {
        let op = *self;
        Some(Box::new(move |i| {
            let x = i[0].f32();
            match op {
                Norm => {
                    let row = *i[0].shape.last().unwrap();
                    Value::new(&op.shape(&[&i[0].shape]), x.chunks(row).map(|d| d.iter().map(|v| v * v).sum::<f32>().sqrt()).collect())
                }
                Cutoff { rc } => Value::new(&i[0].shape, x.iter().map(|r| cut(*r, rc as f32)).collect()),
                Basis { k, rc } => {
                    let mu = centers(k, rc);
                    let g = x.iter().flat_map(|r| mu.iter().map(move |m| (-((r - m) * k as f32 / rc as f32).powi(2)).exp() * cut(*r, rc as f32)));
                    Value::new(&op.shape(&[&i[0].shape]), g.collect())
                }
            }
        }))
    }

    fn cubecl(&self) -> Option<nary::cubecl::Kernel<'_>> {
        let op = *self;
        if let Basis { .. } = op {
            return None;
        }
        Some(Box::new(move |c, i, shape| {
            let out = empty(c, shape);
            let (count, dim) = grid(out.len());
            unsafe {
                match op {
                    Norm => k_norm::launch_unchecked(c, count, dim, arg(i[0]), arg(&out), *i[0].shape.last().unwrap() as u32),
                    Cutoff { rc } => k_cutoff::launch_unchecked(c, count, dim, arg(i[0]), arg(&out), rc as f32),
                    Basis { .. } => unreachable!(),
                }
            }
            out
        }))
    }
}

fn centers(k: usize, rc: f64) -> Vec<f32> {
    (0..k).map(|j| (rc * j as f64 / (k - 1) as f64) as f32).collect()
}

fn cut(r: f32, rc: f32) -> f32 {
    (1.0 - r / rc).max(0.0).powi(2)
}

#[cube(launch_unchecked)]
fn k_norm(x: &[f32], y: &mut [f32], row: u32) {
    if ABSOLUTE_POS < y.len() {
        let mut acc = 0.0f32;
        for c in 0..row as usize {
            let v = x[ABSOLUTE_POS * row as usize + c];
            acc += v * v;
        }
        y[ABSOLUTE_POS] = f32::sqrt(acc);
    }
}

#[cube(launch_unchecked)]
fn k_cutoff(r: &[f32], y: &mut [f32], rc: f32) {
    if ABSOLUTE_POS < y.len() {
        let t = f32::max(1.0 - r[ABSOLUTE_POS] / rc, 0.0);
        y[ABSOLUTE_POS] = t * t;
    }
}
