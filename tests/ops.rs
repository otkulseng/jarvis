//! One check per operation, on each backend against the CPU.

use nary::{array, Array, Backend};
use nary_mlip as mlip;

fn same(a: Array) {
    let cpu = a.get_on(Backend::Cpu);
    for backend in [Backend::StableHlo, Backend::CubeCl] {
        let got = a.get_on(backend);
        assert_eq!(got.shape, cpu.shape);
        for (x, y) in cpu.f32().iter().zip(got.f32()) {
            assert!((x - y).abs() < 1e-4, "cpu {x} vs {backend:?} {y}");
        }
    }
}
fn m() -> Array {
    array([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]])
}

#[test]
fn bias() {
    same(m() + array([0.1, 0.2, 0.3]))
}
#[test]
fn column() {
    same(m() * array([[2.0], [3.0]]))
}
#[test]
fn scalar() {
    same((m() - 1.0).maximum(0.0))
}
#[test]
fn reshape() {
    same(m().expand_dims(1).sum(1))
}
#[test]
fn matmul() {
    same(m().matmul(array([[1.0], [0.5], [0.25]])))
}
#[test]
fn take() {
    same(m()[array([1, 0, 1])])
}
#[test]
fn scatter_rows() {
    same(m().scatter_add(array([1, 1]), 3))
}
#[test]
fn norm() {
    same(mlip::norm(m()))
}
#[test]
fn cutoff() {
    same(mlip::cutoff(array([0.5, 2.0, 4.0]), 3.0))
}
#[test]
fn basis() {
    same(mlip::basis(array([0.5, 2.0, 4.0]), 4, 3.0))
}
