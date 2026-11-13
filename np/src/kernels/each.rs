use crate::Value;

/// `f` of each pair of elements of two arrays, the way NumPy fits their
/// shapes together: aligned from the last axis, an axis of length one (or
/// one that is not there) standing for every element along the other's.
///
/// Shapes that do not fit together are a mistake in the program, and stop it.
pub fn each(symbol: &str, a: &Value, b: &Value, f: impl Fn(f64, f64) -> f64) -> Value {
    if a.shape == b.shape {
        return Value {
            shape: a.shape.clone(),
            data: a.data.iter().zip(&b.data).map(|(a, b)| f(*a, *b)).collect(),
        };
    }
    if a.shape.is_empty() {
        let a = a.data[0];
        return Value {
            shape: b.shape.clone(),
            data: b.data.iter().map(|b| f(a, *b)).collect(),
        };
    }
    if b.shape.is_empty() {
        let b = b.data[0];
        return Value {
            shape: a.shape.clone(),
            data: a.data.iter().map(|a| f(*a, b)).collect(),
        };
    }

    let rank = a.shape.len().max(b.shape.len());
    // The length of an array along one of the `rank` axes.
    let along = |v: &Value, axis: usize| {
        let absent = rank - v.shape.len();
        if axis < absent {
            1
        } else {
            v.shape[axis - absent]
        }
    };
    let shape: Vec<usize> = (0..rank)
        .map(|axis| match (along(a, axis), along(b, axis)) {
            (n, m) if n == m => n,
            (1, m) => m,
            (n, 1) => n,
            _ => panic!(
                "{symbol}: the shapes {:?} and {:?} do not fit together",
                a.shape, b.shape
            ),
        })
        .collect();
    // How far one step along each axis moves in an array's data: nowhere,
    // along an axis where its one element stands for all.
    let strides = |v: &Value| {
        let mut strides = vec![0; rank];
        let mut size = 1;
        for axis in (0..rank).rev() {
            let n = along(v, axis);
            if n != 1 {
                strides[axis] = size;
            }
            size *= n;
        }
        strides
    };
    let (step_a, step_b) = (strides(a), strides(b));

    let total: usize = shape.iter().product();
    let mut data = Vec::with_capacity(total);
    let mut index = vec![0; rank];
    let (mut at_a, mut at_b) = (0, 0);
    for _ in 0..total {
        data.push(f(a.data[at_a], b.data[at_b]));
        for axis in (0..rank).rev() {
            index[axis] += 1;
            at_a += step_a[axis];
            at_b += step_b[axis];
            if index[axis] < shape[axis] {
                break;
            }
            index[axis] = 0;
            at_a -= step_a[axis] * shape[axis];
            at_b -= step_b[axis] * shape[axis];
        }
    }
    Value { shape, data }
}
