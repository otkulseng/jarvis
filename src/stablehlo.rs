//! The StableHLO backend. Each kernel emits the text of its operation. The
//! whole graph becomes one module, which PJRT compiles and runs.

use std::println;

use pjrt::{Client, HostBuffer, LoadedExecutable, Program, ProgramFormat};

use crate::{Data, Graph, Value};

/// Where a kernel writes its operation: the SSA names, types and shapes of
/// its inputs, and the name, type and shape of its result.
pub struct Site<'a> {
    pub ins: Vec<String>,
    pub tys: Vec<String>,
    pub shapes: Vec<&'a [usize]>,
    pub out: String,
    pub out_ty: String,
    pub out_shape: &'a [usize],
}

pub type Kernel<'a> = Box<dyn Fn(&Site) -> String + 'a>;

pub fn ty(shape: &[usize], elem: &str) -> String {
    let dims: String = shape.iter().map(|d| format!("{d}x")).collect();
    format!("tensor<{dims}{elem}>")
}

fn elem(v: &Value) -> &'static str {
    match v.data {
        Data::F32(_) => "f32",
        Data::I32(_) => "i32",
    }
}

pub fn emit(g: &Graph) -> String {
    let mut names: Vec<String> = vec![];
    let mut tys: Vec<String> = vec![];
    let mut body = String::new();
    for (i, node) in g.nodes.iter().enumerate() {
        if let Some(k) = node.input {
            names.push(format!("%in{k}"));
            tys.push(ty(&node.shape, elem(&g.inputs[k])));
            continue;
        }
        let site = Site {
            ins: node.inputs.iter().map(|j| names[*j].clone()).collect(),
            tys: node.inputs.iter().map(|j| tys[*j].clone()).collect(),
            shapes: node.inputs.iter().map(|j| g.nodes[*j].shape.as_slice()).collect(),
            out: format!("%n{i}"),
            out_ty: ty(&node.shape, "f32"),
            out_shape: &node.shape,
        };
        let kernel = node.op.stablehlo().unwrap_or_else(|| panic!("no stablehlo kernel for {}", node.key));
        body += &kernel(&site);
        names.push(site.out);
        tys.push(site.out_ty);
    }
    let args: Vec<String> =
        g.inputs.iter().enumerate().map(|(k, v)| format!("%in{k}: {}", ty(&v.shape, elem(v)))).collect();
    let (root, root_ty) = (&names[g.root], &tys[g.root]);
    format!("func.func @main({}) -> {root_ty} {{\n{body}  return {root} : {root_ty}\n}}\n", args.join(", "))
}

pub fn run(g: &Graph) -> Value {
    let text = emit(g);
    let path = ::std::env::var("PJRT_PLUGIN").unwrap_or(".pjrt/libpjrt_cpu.so".into());
    let api = pjrt::plugin(path).load().expect("load PJRT plugin");
    let client = Client::builder(&api).build().expect("PJRT client");
    let program = Program::new(ProgramFormat::MLIR, text.as_bytes());
    let exe = LoadedExecutable::builder(&client, &program).build().unwrap_or_else(|e| panic!("{e}\n{text}"));
    let inputs: Vec<pjrt::Buffer> = g
        .inputs
        .iter()
        .map(|v| {
            let dims: Vec<i64> = v.shape.iter().map(|d| *d as i64).collect();
            let host = match &v.data {
                Data::F32(x) => HostBuffer::builder().data(x.clone()).dims(dims).build(),
                Data::I32(x) => HostBuffer::builder().data(x.clone()).dims(dims).build(),
            };
            host.copy_to_sync(&client).expect("copy to device")
        })
        .collect();
    let outputs = exe.execution(inputs).run_sync().expect("execute");
    let shape = g.nodes[g.root].shape.clone();
    match outputs[0][0].copy_to_host_sync().expect("copy to host") {
        HostBuffer::F32(b) => Value::new(&shape, b.data().to_vec()),
        other => panic!("unexpected result type {other:?}"),
    }
}
