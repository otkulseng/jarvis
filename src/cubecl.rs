//! The CubeCL backend, on wgpu. One launch per node.

use ::cubecl::prelude::*;
use ::cubecl::server::Handle;
use ::cubecl::wgpu::{AutoCompiler, WgpuDevice, WgpuRuntime};
use cubecl_runtime::runtime::Runtime;

use crate::{Data, Graph, Value};

pub use ::cubecl::client::Client;

/// A device buffer and the shape it holds.
pub struct Buf {
    pub handle: Handle,
    pub shape: Vec<usize>,
}

impl Buf {
    pub fn len(&self) -> usize {
        self.shape.iter().product()
    }
}

/// Runs the operation on the client over its inputs, into a buffer of the given shape.
pub type Kernel<'a> = Box<dyn Fn(&Client, &[&Buf], &[usize]) -> Buf + 'a>;

pub fn empty(client: &Client, shape: &[usize]) -> Buf {
    Buf { handle: client.empty(shape.iter().product::<usize>() * 4), shape: shape.to_vec() }
}

/// One unit per element, in cubes of 256.
pub fn grid(len: usize) -> (CubeCount, CubeDim) {
    (CubeCount::Static(len.div_ceil(256).max(1) as u32, 1, 1), CubeDim::new_1d(256))
}

pub fn arg(b: &Buf) -> BufferArg {
    unsafe { BufferArg::from_raw_parts(b.handle.clone(), b.len()) }
}

pub fn run(g: &Graph) -> Value {
    let client = WgpuRuntime::<AutoCompiler>::client(&WgpuDevice::default());
    let mut vals: Vec<Buf> = vec![];
    for node in &g.nodes {
        let b = if let Some(k) = node.input {
            let bytes = match &g.inputs[k].data {
                Data::F32(v) => f32::as_bytes(v),
                Data::I32(v) => i32::as_bytes(v),
            };
            Buf { handle: client.create_from_slice(bytes), shape: g.inputs[k].shape.clone() }
        } else {
            let ins: Vec<&Buf> = node.inputs.iter().map(|i| &vals[*i]).collect();
            let kernel = node.op.cubecl().unwrap_or_else(|| panic!("no cubecl kernel for {}", node.key));
            kernel(&client, &ins, &node.shape)
        };
        vals.push(b);
    }
    let bytes = client.read_one_unchecked(vals[g.root].handle.clone());
    Value::new(&g.nodes[g.root].shape, f32::from_bytes(&bytes).to_vec())
}
