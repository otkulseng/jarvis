//! A small potential: each atom's neighbours are expanded in a radial basis,
//! summed into a feature vector, and sent through a two-layer network. The
//! same program runs on every backend. `NARY_BACKEND` picks one.

use nary::array;
use nary_mlip as mlip;

fn main() {
    let pos = array([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.5, 0.0], [1.0, 1.0, 2.0]]);
    let i = array([0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3]);
    let j = array([1, 2, 3, 0, 2, 3, 0, 1, 3, 0, 1, 2]);

    let d = pos[j] - pos[i];
    let r = mlip::norm(d);
    let g = mlip::basis(r, 4, 3.0);
    let feat = g.scatter_add(i, 4);

    let w1 = array([[0.5, -0.2, 0.1], [0.3, 0.8, -0.5], [-0.7, 0.4, 0.9], [0.2, -0.6, 0.3]]);
    let b1 = array([0.1, -0.1, 0.05]);
    let w2 = array([[1.0], [-0.5], [0.25]]);
    let h = (feat.matmul(w1) + b1).maximum(0.0);
    let per_atom = h.matmul(w2).sum(1);
    let energy = per_atom.sum(0);

    println!("features {:?}", feat.get().f32());
    println!("per atom {:?}", per_atom.get().f32());
    println!("energy {:?}", energy.get().f32());
}
