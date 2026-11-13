//! The backends must agree. Each backend lacks some kernel and runs that
//! operation's equivalent instead, so this also checks the equivalences.

use nary::{array, Array, Backend};
use nary_mlip as mlip;

fn per_atom() -> Array {
    let pos = array([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.5, 0.0], [1.0, 1.0, 2.0]]);
    let i = array([0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3]);
    let j = array([1, 2, 3, 0, 2, 3, 0, 1, 3, 0, 1, 2]);
    let feat = mlip::basis(mlip::norm(pos[j] - pos[i]), 4, 3.0).scatter_add(i, 4);
    let w1 = array([[0.5, -0.2, 0.1], [0.3, 0.8, -0.5], [-0.7, 0.4, 0.9], [0.2, -0.6, 0.3]]);
    let b1 = array([0.1, -0.1, 0.05]);
    let w2 = array([[1.0], [-0.5], [0.25]]);
    (feat.matmul(w1) + b1).maximum(0.0).matmul(w2).sum(1)
}

#[test]
fn backends_agree() {
    let e = per_atom();
    let cpu = e.get_on(Backend::Cpu);
    for backend in [Backend::StableHlo, Backend::CubeCl] {
        let got = e.get_on(backend);
        assert_eq!(got.shape, cpu.shape);
        for (a, b) in got.f32().iter().zip(cpu.f32()) {
            assert!((a - b).abs() < 1e-4, "{backend:?}: {a} vs cpu {b}");
        }
    }
}
