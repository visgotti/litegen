//! The small amount of linear algebra every codec needs, in one place.
//!
//! Each container states transforms its own way — glTF as TRS or a column-major
//! matrix, FBX as Euler degrees with pivots and an explicit rotation order, USD
//! as an ordered stack of ops, Collada as a matrix or a list of rotations, 3MF
//! as a 3x4 affine. They all have to meet in one convention, and the bugs this
//! module exists to prevent are the quiet ones: a row-major matrix read as
//! column-major renders a sheared model, an Euler order applied backwards
//! rotates every animated joint the wrong way, and both look plausible.
//!
//! # Conventions, fixed here and nowhere else
//!
//! - Matrices are 4x4, column-major, `f64`: element (row `r`, column `c`) lives
//!   at `[c * 4 + r]`, exactly as glTF stores `node.matrix`. Composition happens
//!   in `f64` because node chains multiply, and a five-deep chain in `f32` drifts
//!   enough on large coordinates that seams stop welding.
//! - Vectors are columns: `transform_point(m, p)` is `m * p`. `mat_mul(a, b)`
//!   applies `b` first.
//! - Quaternions are `[x, y, z, w]`, unit length, glTF's order.
//! - Euler angles are radians unless a function says degrees, and an
//!   [`EulerOrder`] names the order the rotations are APPLIED in: `Xyz` rotates
//!   about X first, then Y, then Z, i.e. `R = Rz * Ry * Rx`. That is FBX's
//!   `eEulerXYZ` and Collada's usual `rotateZ rotateY rotateX` stack.

pub type Mat4 = [f64; 16];

pub const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

pub const QUAT_IDENTITY: [f64; 4] = [0.0, 0.0, 0.0, 1.0];

/// `a * b`, i.e. apply `b` first.
pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f64; 16];
    for c in 0..4 {
        for r in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + r] * b[c * 4 + k];
            }
            out[c * 4 + r] = sum;
        }
    }
    out
}

pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
    let mut out = [0.0f32; 3];
    for r in 0..3 {
        out[r] = (m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r]) as f32;
    }
    out
}

pub fn transform_point_f64(m: &Mat4, p: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0f64; 3];
    for r in 0..3 {
        out[r] = m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r];
    }
    out
}

/// Upper-left 3x3, column-major, as `[c * 3 + r]`.
pub fn upper3(m: &Mat4) -> [f64; 9] {
    let mut u = [0.0f64; 9];
    for c in 0..3 {
        for r in 0..3 {
            u[c * 3 + r] = m[c * 4 + r];
        }
    }
    u
}

pub fn det3(u: &[f64; 9]) -> f64 {
    u[0] * (u[4] * u[8] - u[7] * u[5]) - u[3] * (u[1] * u[8] - u[7] * u[2])
        + u[6] * (u[1] * u[5] - u[4] * u[2])
}

/// Whether this transform mirrors space, which reverses which side of every
/// triangle faces out. Every format that bakes transforms into vertices has to
/// flip the winding when this is true, or a mirrored part renders inside out.
pub fn is_mirroring(m: &Mat4) -> bool {
    det3(&upper3(m)) < 0.0
}

/// The matrix that carries normals: inverse-transpose of the upper 3x3.
///
/// The plain 3x3 is only correct under uniform scale; vendors bake non-uniform
/// scale into root nodes often enough (a mirrored `[-1, 1, 1]` is the usual
/// one) that using it would tilt normals on stretched geometry. A singular
/// transform has no right answer, so it falls back to the plain 3x3 rather
/// than propagate NaN.
pub fn normal_matrix(m: &Mat4) -> [f64; 9] {
    let u = upper3(m);
    let det = det3(&u);
    if det.abs() < 1e-12 || !det.is_finite() {
        return u;
    }
    let inv = [
        (u[4] * u[8] - u[7] * u[5]) / det,
        (u[7] * u[2] - u[1] * u[8]) / det,
        (u[1] * u[5] - u[4] * u[2]) / det,
        (u[6] * u[5] - u[3] * u[8]) / det,
        (u[0] * u[8] - u[6] * u[2]) / det,
        (u[3] * u[2] - u[0] * u[5]) / det,
        (u[3] * u[7] - u[6] * u[4]) / det,
        (u[6] * u[1] - u[0] * u[7]) / det,
        (u[0] * u[4] - u[3] * u[1]) / det,
    ];
    let mut out = [0.0f64; 9];
    for c in 0..3 {
        for r in 0..3 {
            out[c * 3 + r] = inv[r * 3 + c];
        }
    }
    out
}

/// Apply a 3x3 (as from [`normal_matrix`]) to a direction and renormalise.
pub fn transform_normal(u: &[f64; 9], n: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (n[0] as f64, n[1] as f64, n[2] as f64);
    let mut v = [0.0f64; 3];
    for r in 0..3 {
        v[r] = u[r] * x + u[3 + r] * y + u[6 + r] * z;
    }
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 && len.is_finite() {
        [(v[0] / len) as f32, (v[1] / len) as f32, (v[2] / len) as f32]
    } else {
        // A normal that collapsed under the transform carries no direction;
        // the input is the least-wrong thing to keep, and keeps array lengths.
        n
    }
}

/// Apply a 3x3 to a direction without renormalising (morph-target deltas, which
/// are offsets, not unit vectors).
pub fn transform_dir3(u: &[f64; 9], v: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (v[0] as f64, v[1] as f64, v[2] as f64);
    let mut out = [0.0f32; 3];
    for r in 0..3 {
        out[r] = (u[r] * x + u[3 + r] * y + u[6 + r] * z) as f32;
    }
    out
}

/// General 4x4 inverse. `None` for a singular matrix.
pub fn invert(m: &Mat4) -> Option<Mat4> {
    // Cofactor expansion, written out: generic Gaussian elimination is longer
    // and no more accurate at this size.
    let a = m;
    let mut inv = [0.0f64; 16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15]
        + a[9] * a[7] * a[14] + a[13] * a[6] * a[11] - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15]
        - a[8] * a[7] * a[14] - a[12] * a[6] * a[11] + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15]
        + a[8] * a[7] * a[13] + a[12] * a[5] * a[11] - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14]
        - a[8] * a[6] * a[13] - a[12] * a[5] * a[10] + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15]
        - a[9] * a[3] * a[14] - a[13] * a[2] * a[11] + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15]
        + a[8] * a[3] * a[14] + a[12] * a[2] * a[11] - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15]
        - a[8] * a[3] * a[13] - a[12] * a[1] * a[11] + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14]
        + a[8] * a[2] * a[13] + a[12] * a[1] * a[10] - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15]
        + a[5] * a[3] * a[14] + a[13] * a[2] * a[7] - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15]
        - a[4] * a[3] * a[14] - a[12] * a[2] * a[7] + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15]
        + a[4] * a[3] * a[13] + a[12] * a[1] * a[7] - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14]
        - a[4] * a[2] * a[13] - a[12] * a[1] * a[6] + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11]
        - a[5] * a[3] * a[10] - a[9] * a[2] * a[7] + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11]
        + a[4] * a[3] * a[10] + a[8] * a[2] * a[7] - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11]
        - a[4] * a[3] * a[9] - a[8] * a[1] * a[7] + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10]
        + a[4] * a[2] * a[9] + a[8] * a[1] * a[6] - a[8] * a[2] * a[5];
    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    if det.abs() < 1e-300 || !det.is_finite() {
        return None;
    }
    let inv_det = 1.0 / det;
    for v in inv.iter_mut() {
        *v *= inv_det;
    }
    Some(inv)
}

pub fn transpose(m: &Mat4) -> Mat4 {
    let mut out = [0.0f64; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[r * 4 + c] = m[c * 4 + r];
        }
    }
    out
}

pub fn translation(t: [f64; 3]) -> Mat4 {
    let mut m = IDENTITY;
    m[12] = t[0];
    m[13] = t[1];
    m[14] = t[2];
    m
}

pub fn scaling(s: [f64; 3]) -> Mat4 {
    let mut m = IDENTITY;
    m[0] = s[0];
    m[5] = s[1];
    m[10] = s[2];
    m
}

/// Rotation matrix of a unit quaternion.
pub fn quat_to_mat(q: [f64; 4]) -> Mat4 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        0.0,
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        0.0,
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// TRS composed the way glTF defines it: `M = T * R * S`.
pub fn compose(t: [f64; 3], q: [f64; 4], s: [f64; 3]) -> Mat4 {
    let r = quat_to_mat(q);
    [
        r[0] * s[0], r[1] * s[0], r[2] * s[0], 0.0, //
        r[4] * s[1], r[5] * s[1], r[6] * s[1], 0.0, //
        r[8] * s[2], r[9] * s[2], r[10] * s[2], 0.0, //
        t[0], t[1], t[2], 1.0,
    ]
}

/// Quaternion of a pure rotation matrix (orthonormal upper 3x3, det +1).
///
/// Shepperd's method: branch on the largest diagonal term so the square root is
/// never taken of a small, cancellation-damaged number.
pub fn mat3_to_quat(u: &[f64; 9]) -> [f64; 4] {
    // u is column-major [c*3 + r]; name the elements by row/column.
    let (m00, m01, m02) = (u[0], u[3], u[6]);
    let (m10, m11, m12) = (u[1], u[4], u[7]);
    let (m20, m21, m22) = (u[2], u[5], u[8]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    };
    quat_normalize(q)
}

/// Split an affine matrix into translation, rotation and scale.
///
/// Returns `None` when the matrix has no TRS form: a projective bottom row,
/// a non-finite entry, or shear in the upper 3x3. A writer that needs TRS
/// must then say so rather than emit a rotation that is only nearly right. A
/// mirror is folded into a negative X scale, which is how glTF, USD and FBX
/// all express it. Degenerate scales decompose; see [`decompose_near`].
pub fn decompose(m: &Mat4) -> Option<([f64; 3], [f64; 4], [f64; 3])> {
    decompose_near(m, None)
}

/// [`decompose`], with a rotation to fall back on.
///
/// A matrix with an axis scaled to nothing (a part hidden, or grown in from
/// nothing, as rigs and pop-in animations do) is singular but still exactly
/// T·R·S: that axis takes zero scale, and the directions it no longer defines
/// come from the remaining axes or, where those leave a choice, from `near` —
/// the previous key's rotation in an animation, so the key interpolates from
/// where the animation was — else from the identity.
pub fn decompose_near(m: &Mat4, near: Option<[f64; 4]>) -> Option<([f64; 3], [f64; 4], [f64; 3])> {
    if m[3].abs() > 1e-9 || m[7].abs() > 1e-9 || m[11].abs() > 1e-9 || (m[15] - 1.0).abs() > 1e-9 {
        // A projective bottom row is not a node transform at all.
        return None;
    }
    if m.iter().any(|v| !v.is_finite()) {
        return None;
    }
    // An axis shorter than this has no direction worth keeping.
    const ZERO: f64 = 1e-12;
    // Orthogonality tolerance: exporters write matrices through f32, so an
    // honest rotation is only orthogonal to ~1e-6. Anything well past that is
    // genuine shear.
    const SHEAR_EPS: f64 = 1e-4;
    let col = |c: usize| [m[c * 4], m[c * 4 + 1], m[c * 4 + 2]];
    let len = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let t = [m[12], m[13], m[14]];
    let mut scale = [0.0f64; 3];
    let mut axes: [Option<[f64; 3]>; 3] = [None; 3];
    for c in 0..3 {
        let v = col(c);
        scale[c] = len(v);
        if scale[c] > ZERO {
            axes[c] = Some([v[0] / scale[c], v[1] / scale[c], v[2] / scale[c]]);
        }
    }
    // What is left must still be orthogonal: shear is not a degenerate scale.
    for a in 0..3 {
        for b in a + 1..3 {
            if let (Some(x), Some(y)) = (axes[a], axes[b]) {
                if dot(x, y).abs() > SHEAR_EPS {
                    return None;
                }
            }
        }
    }
    let hint = quat_to_mat(near.unwrap_or(QUAT_IDENTITY));
    let hint_axis = |c: usize| [hint[c * 4], hint[c * 4 + 1], hint[c * 4 + 2]];
    // One surviving axis: borrow a second from the hint, made orthogonal to it.
    if axes.iter().filter(|a| a.is_some()).count() == 1 {
        let (k, x) = axes.iter().enumerate().find_map(|(k, a)| a.map(|a| (k, a)))?;
        let other = (k + 1) % 3;
        let mut y = hint_axis(other);
        let d = dot(x, y);
        y = [y[0] - d * x[0], y[1] - d * x[1], y[2] - d * x[2]];
        let l = len(y);
        if l <= 1e-9 {
            // The hint lines up with the survivor; any perpendicular serves,
            // and this one is deterministic.
            let pick = if x[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            let c = cross(x, pick);
            let lc = len(c);
            axes[other] = Some([c[0] / lc, c[1] / lc, c[2] / lc]);
        } else {
            axes[other] = Some([y[0] / l, y[1] / l, y[2] / l]);
        }
    }
    // Two axes: the third is their cross product, in right-handed order.
    match axes {
        [Some(_), Some(_), Some(_)] => {}
        [None, Some(y), Some(z)] => axes[0] = Some(cross(y, z)),
        [Some(x), None, Some(z)] => axes[1] = Some(cross(z, x)),
        [Some(x), Some(y), None] => axes[2] = Some(cross(x, y)),
        // Every axis collapsed: a point, rotated as it was.
        _ => return Some((t, near.unwrap_or(QUAT_IDENTITY), [0.0; 3])),
    }
    let [x, y, z] = axes.map(|a| a.unwrap_or([0.0; 3]));
    let mut r = [x[0], x[1], x[2], y[0], y[1], y[2], z[0], z[1], z[2]];
    if dot(cross(x, y), z) < 0.0 {
        // A mirror, folded into a negative X scale.
        scale[0] = -scale[0];
        for v in r.iter_mut().take(3) {
            *v = -*v;
        }
    }
    Some((t, mat3_to_quat(&r), scale))
}

/// A scale with every exactly-zero (or smaller than a millionth) component
/// written as a millionth, keeping its sign.
///
/// A part scaled to nothing is a singular matrix, and three.js (r183)
/// decomposes a singular matrix to UNIT scale and no rotation
/// (`Matrix4.decompose`: `if ( det === 0 )`). Its Collada and FBX loaders
/// decompose every node transform, so a hidden part comes back full size. A
/// millionth renders the same as nothing and decomposes.
pub fn nonsingular_scale(s: [f64; 3]) -> [f64; 3] {
    const MIN: f64 = 1e-6;
    s.map(|v| if v.abs() < MIN { MIN.copysign(v) } else { v })
}

pub fn quat_normalize(q: [f64; 4]) -> [f64; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len > 0.0 && len.is_finite() {
        [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
    } else {
        QUAT_IDENTITY
    }
}

/// Hamilton product `a * b`: rotate by `b`, then by `a`.
pub fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub fn quat_dot(a: [f64; 4], b: [f64; 4]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// Spherical interpolation along the shorter arc — glTF's LINEAR for rotations.
pub fn slerp(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    let mut b = b;
    let mut d = quat_dot(a, b);
    if d < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        d = -d;
    }
    if d > 0.9995 {
        // Nearly parallel: the sine below vanishes and nlerp is exact to
        // within f64 noise.
        let q = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
            a[3] + (b[3] - a[3]) * t,
        ];
        return quat_normalize(q);
    }
    let theta = d.clamp(-1.0, 1.0).acos();
    let sin = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin;
    let wb = (t * theta).sin() / sin;
    quat_normalize([
        a[0] * wa + b[0] * wb,
        a[1] * wa + b[1] * wb,
        a[2] * wa + b[2] * wb,
        a[3] * wa + b[3] * wb,
    ])
}

pub fn quat_from_axis_angle(axis: [f64; 3], angle: f64) -> [f64; 4] {
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if !(len > 0.0) || !len.is_finite() {
        return QUAT_IDENTITY;
    }
    let (s, c) = (angle * 0.5).sin_cos();
    [axis[0] / len * s, axis[1] / len * s, axis[2] / len * s, c]
}

/// The order Euler rotations are applied in (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EulerOrder {
    Xyz,
    Xzy,
    Yzx,
    Yxz,
    Zxy,
    Zyx,
}

impl EulerOrder {
    /// The axes in application order: `Xyz` → `[0, 1, 2]`.
    pub fn axes(self) -> [usize; 3] {
        match self {
            EulerOrder::Xyz => [0, 1, 2],
            EulerOrder::Xzy => [0, 2, 1],
            EulerOrder::Yzx => [1, 2, 0],
            EulerOrder::Yxz => [1, 0, 2],
            EulerOrder::Zxy => [2, 0, 1],
            EulerOrder::Zyx => [2, 1, 0],
        }
    }
}

fn axis_quat(axis: usize, angle: f64) -> [f64; 4] {
    let mut a = [0.0; 3];
    a[axis] = 1.0;
    quat_from_axis_angle(a, angle)
}

/// Quaternion for Euler angles `e = [x, y, z]` (radians) applied in `order`.
pub fn euler_to_quat(e: [f64; 3], order: EulerOrder) -> [f64; 4] {
    let [first, second, third] = order.axes();
    // Applied first means rightmost in the product.
    let q = quat_mul(axis_quat(third, e[third]), quat_mul(axis_quat(second, e[second]), axis_quat(first, e[first])));
    quat_normalize(q)
}

/// Euler angles (radians, `[x, y, z]`) that reproduce `q` when applied in
/// `order`. In gimbal lock the split between the two aligned axes is
/// arbitrary; this puts it all on the first axis applied.
pub fn quat_to_euler(q: [f64; 4], order: EulerOrder) -> [f64; 3] {
    let m = quat_to_mat(quat_normalize(q));
    // Row/column accessor on the column-major matrix.
    let at = |r: usize, c: usize| m[c * 4 + r];
    let [i, j, k] = order.axes();
    // R = R_k(c) * R_j(b) * R_i(a), with (i, j, k) a permutation of (0, 1, 2).
    // The sign of the permutation decides the signs in the closed form.
    let parity = if (j + 3 - i) % 3 == 1 { 1.0 } else { -1.0 };
    let sb = (-parity * at(k, i)).clamp(-1.0, 1.0);
    let b = sb.asin();
    let (a, c);
    if sb.abs() < 1.0 - 1e-12 {
        a = (parity * at(k, j)).atan2(at(k, k));
        c = (parity * at(j, i)).atan2(at(i, i));
    } else {
        // Gimbal lock: the first and third axes coincide.
        c = 0.0;
        a = (-parity * at(j, k)).atan2(at(j, j));
    }
    let mut out = [0.0; 3];
    out[i] = a;
    out[j] = b;
    out[k] = c;
    out
}

/// Make an Euler triple continuous with the previous key: of the infinitely
/// many equivalent spellings, pick the one nearest `prev`. Without this an
/// animated rotation crossing ±180° gets keyed as a jump of 360°, and a reader
/// interpolating per component spins the joint the long way round.
pub fn euler_unwrap(e: [f64; 3], prev: [f64; 3], order: EulerOrder) -> [f64; 3] {
    use std::f64::consts::PI;
    let wrap = |a: f64, p: f64| a + ((p - a) / (2.0 * PI)).round() * 2.0 * PI;
    let direct = [wrap(e[0], prev[0]), wrap(e[1], prev[1]), wrap(e[2], prev[2])];
    // The alternative spelling of the same rotation: (a+π, π-b, c+π) on the
    // order's own axes.
    let [i, j, k] = order.axes();
    let mut alt = [0.0; 3];
    alt[i] = e[i] + PI;
    alt[j] = PI - e[j];
    alt[k] = e[k] + PI;
    let alt = [wrap(alt[0], prev[0]), wrap(alt[1], prev[1]), wrap(alt[2], prev[2])];
    let dist = |v: [f64; 3]| (v[0] - prev[0]).abs() + (v[1] - prev[1]).abs() + (v[2] - prev[2]).abs();
    if dist(alt) < dist(direct) {
        alt
    } else {
        direct
    }
}

/// sRGB transfer, for the formats whose colours are display-referred.
///
/// Computed in f64 with the endpoints pinned: in f32, `1.055 * 1^(1/2.4) -
/// 0.055` is 0.99999994, which a text format then writes out as such and a
/// byte format rounds the wrong way often enough to matter.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.0 {
        return 0.0;
    }
    if c >= 1.0 {
        return 1.0;
    }
    let c = c as f64;
    (if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }) as f32
}

pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0 {
        return 0.0;
    }
    if c >= 1.0 {
        return 1.0;
    }
    let c = c as f64;
    (if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &[f64], b: &[f64], eps: f64) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < eps)
    }

    fn quat_close(a: [f64; 4], b: [f64; 4]) -> bool {
        // q and -q are the same rotation.
        quat_dot(a, b).abs() > 1.0 - 1e-9
    }

    #[test]
    fn compose_then_decompose_is_identity_for_trs() {
        let t = [1.5, -2.0, 3.25];
        let q = quat_normalize([0.3, -0.5, 0.2, 0.8]);
        let s = [2.0, 0.5, 3.0];
        let (t2, q2, s2) = decompose(&compose(t, q, s)).expect("no shear");
        assert!(close(&t, &t2, 1e-12), "{t2:?}");
        assert!(quat_close(q, q2), "{q2:?}");
        assert!(close(&s, &s2, 1e-12), "{s2:?}");
    }

    #[test]
    fn decompose_folds_a_mirror_into_negative_x_scale() {
        let m = compose([0.0; 3], quat_normalize([0.0, 0.0, 0.38, 0.92]), [1.0, -1.0, 1.0]);
        let (_, q, s) = decompose(&m).unwrap();
        assert!(s[0] < 0.0 && s[1] > 0.0 && s[2] > 0.0, "{s:?}");
        // Whatever spelling it picked, it must rebuild the same matrix.
        assert!(close(&compose([0.0; 3], q, s), &m, 1e-9));
    }

    #[test]
    fn degenerate_scales_decompose_exactly_and_shear_still_does_not() {
        let q = quat_from_axis_angle([0.3, 0.8, 0.1], 1.1);
        for scale in [[0.0, 2.0, 3.0], [2.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, -1.0, 2.0], [1.0, 1e-13, 1.0]] {
            let m = compose([1.0, 2.0, 3.0], q, scale);
            for near in [None, Some(q)] {
                let (t, r, s) = decompose_near(&m, near).unwrap_or_else(|| panic!("{scale:?}"));
                let back = compose(t, r, s);
                assert!(back.iter().zip(&m).all(|(a, b)| (a - b).abs() < 1e-9), "{scale:?}: {back:?} vs {m:?}");
                assert!(s.iter().zip(&scale).all(|(a, b)| (a.abs() - b.abs()).abs() < 1e-9), "{scale:?} -> {s:?}");
            }
            // With the true rotation as the hint, a collapsed axis keeps it.
            let (_, r, _) = decompose_near(&m, Some(q)).unwrap();
            if scale.iter().filter(|v| v.abs() > 1e-9).count() >= 1 && scale[1] >= 0.0 {
                assert!(quat_dot(r, q).abs() > 1.0 - 1e-9, "{scale:?}: {r:?} vs {q:?}");
            }
        }
        let mut shear = IDENTITY;
        shear[4] = 0.5;
        shear[10] = 0.0;
        assert!(decompose(&shear).is_none());
    }

    #[test]
    fn decompose_refuses_shear() {
        let mut m = IDENTITY;
        m[4] = 0.5; // x += 0.5 * y
        assert!(decompose(&m).is_none());
    }

    #[test]
    fn invert_round_trips() {
        let m = compose([1.0, 2.0, 3.0], quat_normalize([0.1, 0.7, -0.2, 0.6]), [2.0, 3.0, 0.5]);
        let inv = invert(&m).unwrap();
        assert!(close(&mat_mul(&m, &inv), &IDENTITY, 1e-12));
        assert!(invert(&scaling([1.0, 0.0, 1.0])).is_none());
    }

    #[test]
    fn euler_round_trips_through_every_order() {
        let orders = [
            EulerOrder::Xyz,
            EulerOrder::Xzy,
            EulerOrder::Yzx,
            EulerOrder::Yxz,
            EulerOrder::Zxy,
            EulerOrder::Zyx,
        ];
        let angles = [[0.3, -0.7, 1.1], [-2.5, 0.4, 0.9], [0.0, 0.0, 0.0], [1.2, 1.4, -3.0]];
        for order in orders {
            for e in angles {
                let q = euler_to_quat(e, order);
                let back = quat_to_euler(q, order);
                assert!(quat_close(q, euler_to_quat(back, order)), "{order:?} {e:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn euler_xyz_applies_x_first() {
        // Rotating +Y by 90° about X gives +Z; then 90° about Z leaves +Z alone.
        // Applied in the other order, +Y would first go to -X and stay in the
        // XY plane. This pins which end of the product "first" is.
        let half = std::f64::consts::FRAC_PI_2;
        let q = euler_to_quat([half, 0.0, half], EulerOrder::Xyz);
        let p = transform_point(&quat_to_mat(q), [0.0, 1.0, 0.0]);
        assert!((p[2] - 1.0).abs() < 1e-6 && p[0].abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn euler_unwrap_avoids_the_long_way_round() {
        use std::f64::consts::PI;
        let prev = [0.0, 0.0, PI - 0.05];
        let next = [0.0, 0.0, -PI + 0.05]; // the same neighbourhood, wrapped
        let u = euler_unwrap(next, prev, EulerOrder::Xyz);
        assert!((u[2] - prev[2]).abs() < 0.2, "{u:?}");
        // And it is still the same rotation.
        assert!(quat_close(euler_to_quat(u, EulerOrder::Xyz), euler_to_quat(next, EulerOrder::Xyz)));
    }

    #[test]
    fn slerp_takes_the_short_arc_and_hits_its_endpoints() {
        let a = quat_from_axis_angle([0.0, 1.0, 0.0], 0.1);
        let b = quat_from_axis_angle([0.0, 1.0, 0.0], 1.1);
        let neg_b = [-b[0], -b[1], -b[2], -b[3]];
        assert!(quat_close(slerp(a, neg_b, 0.0), a));
        assert!(quat_close(slerp(a, neg_b, 1.0), b));
        let mid = slerp(a, neg_b, 0.5);
        assert!(quat_close(mid, quat_from_axis_angle([0.0, 1.0, 0.0], 0.6)), "{mid:?}");
    }

    #[test]
    fn srgb_round_trips_every_eight_bit_code() {
        for n in 0..=255u32 {
            let c = n as f32 / 255.0;
            let back = (linear_to_srgb(srgb_to_linear(c)) * 255.0).round() as u32;
            assert_eq!(back, n);
        }
    }
}
