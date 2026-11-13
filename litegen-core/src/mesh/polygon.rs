//! One polygon as triangles, keeping its winding: what every reader that
//! takes n-gons uses (OBJ `f`, PLY faces, FBX polygons, USD
//! `faceVertexCounts`, Collada `<polylist>`/`<polygons>`).
//!
//! A convex polygon is fanned from its first corner. A concave one is
//! ear-clipped: a fan of it covers area outside the polygon and leaves its
//! notches uncovered, and some of its triangles face backwards — a wrong
//! model, not a cosmetic (2026-10-12 review, finding 20). ufbx and three.js's
//! FBX and USD loaders ear-clip too.

/// Polygons up to this size are ear-clipped when concave; larger concave ones
/// are fanned (and reported by the caller), so a hostile n-gon cannot cost
/// O(n³).
pub const MAX_EAR_CLIP: usize = 64;

pub enum Triangulation {
    /// Convex (or degenerate): fan from the first corner.
    Fan,
    Ears(Vec<[usize; 3]>),
    /// Concave and larger than [`MAX_EAR_CLIP`].
    TooLarge,
}

/// Triangles for one planar-ish polygon, as corner indices into `pts`,
/// keeping its winding.
pub fn triangulate(pts: &[[f64; 3]]) -> Triangulation {
    let n = pts.len();
    // Newell's normal, robust for non-planar input.
    let mut nrm = [0.0f64; 3];
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        nrm[0] += (a[1] - b[1]) * (a[2] + b[2]);
        nrm[1] += (a[2] - b[2]) * (a[0] + b[0]);
        nrm[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
    if !(len > 1e-20) {
        return Triangulation::Fan;
    }
    // Project onto the plane most facing the normal, oriented so the polygon
    // is counter-clockwise in 2-D.
    let axis = if nrm[0].abs() >= nrm[1].abs() && nrm[0].abs() >= nrm[2].abs() {
        0
    } else if nrm[1].abs() >= nrm[2].abs() {
        1
    } else {
        2
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let sign = if nrm[axis] < 0.0 { -1.0 } else { 1.0 };
    let p2: Vec<[f64; 2]> = pts.iter().map(|p| [p[u], p[v] * sign]).collect();
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let convex = (0..n).all(|i| cross(p2[(i + n - 1) % n], p2[i], p2[(i + 1) % n]) >= -1e-12 * len);
    if convex {
        return Triangulation::Fan;
    }
    if n > MAX_EAR_CLIP {
        return Triangulation::TooLarge;
    }
    let mut ring: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let same = |p: [f64; 2], q: [f64; 2]| p[0] == q[0] && p[1] == q[1];
    // A point on the candidate ear's boundary blocks it, as one inside does:
    // clipping past it would cover part of the outline. A point AT one of
    // the ear's own corners does not — a repeated corner (or a keyhole's
    // bridge) is the same place, and counting it made every ear touching it
    // fail, so the whole polygon fell back to a fan.
    let inside = |p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        !(same(p, a) || same(p, b) || same(p, c)) && cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
    };
    let mut guard = 0;
    while ring.len() > 3 && guard < n * n {
        guard += 1;
        let m = ring.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (ring[(k + m - 1) % m], ring[k], ring[(k + 1) % m]);
            let (a, b, c) = (p2[ia], p2[ib], p2[ic]);
            let turn = cross(a, b, c);
            // A repeated corner is a zero-area "ear": clipping it removes the
            // duplicate without covering anything.
            if turn < 0.0 || (turn == 0.0 && !(same(a, b) || same(b, c))) {
                continue;
            }
            if ring.iter().any(|&j| j != ia && j != ib && j != ic && inside(p2[j], a, b, c)) {
                continue;
            }
            out.push([ia, ib, ic]);
            ring.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // Self-intersecting: no ear exists. Fan what is left.
            break;
        }
    }
    for k in 1..ring.len().saturating_sub(1) {
        out.push([ring[0], ring[k], ring[k + 1]]);
    }
    Triangulation::Ears(out)
}

/// [`triangulate`] as a triangle list over `pts`' indices, and whether the
/// polygon was concave past [`MAX_EAR_CLIP`] and so fanned anyway.
pub fn triangles(pts: &[[f64; 3]]) -> (Vec<[usize; 3]>, bool) {
    let fan = |n: usize| (1..n.saturating_sub(1)).map(|k| [0, k, k + 1]).collect::<Vec<_>>();
    match triangulate(pts) {
        Triangulation::Fan => (fan(pts.len()), false),
        Triangulation::Ears(ears) => (ears, false),
        Triangulation::TooLarge => (fan(pts.len()), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(pts: &[[f64; 3]], tris: &[[usize; 3]]) -> Vec<f64> {
        tris.iter()
            .map(|t| {
                let (a, b, c) = (pts[t[0]], pts[t[1]], pts[t[2]]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0
            })
            .collect()
    }

    #[test]
    fn a_square_is_fanned_and_a_dart_is_clipped_at_its_ears() {
        let square = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        assert!(matches!(triangulate(&square), Triangulation::Fan));
        let pentagon = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 2.0, 0.0], [2.0, 0.5, 0.0], [0.0, 2.0, 0.0]];
        let (tris, fanned) = triangles(&pentagon);
        assert!(!fanned);
        let a = area(&pentagon, &tris);
        assert!(a.iter().all(|x| *x > 0.0) && (a.iter().sum::<f64>() - 5.0).abs() < 1e-12, "{a:?}");
    }

    #[test]
    fn a_repeated_corner_does_not_block_every_ear() {
        let c = [[0.0, 0.0], [3.0, 0.0], [3.0, 1.0], [1.0, 1.0], [1.0, 1.0], [1.0, 2.0], [3.0, 2.0], [3.0, 3.0], [0.0, 3.0]];
        let pts: Vec<[f64; 3]> = c.iter().map(|p| [p[0], p[1], 0.0]).collect();
        let (tris, _) = triangles(&pts);
        let a = area(&pts, &tris);
        assert!(a.iter().all(|x| *x >= 0.0) && (a.iter().sum::<f64>() - 7.0).abs() < 1e-12, "{a:?}");
    }

    #[test]
    fn a_clockwise_polygon_keeps_its_winding() {
        let pentagon = [[0.0, 2.0, 0.0], [2.0, 0.5, 0.0], [4.0, 2.0, 0.0], [4.0, 0.0, 0.0], [0.0, 0.0, 0.0]];
        let (tris, _) = triangles(&pentagon);
        let a = area(&pentagon, &tris);
        assert!(a.iter().all(|x| *x < 0.0) && (a.iter().sum::<f64>() + 5.0).abs() < 1e-12, "{a:?}");
    }
}
