//! The format-neutral mesh every reader produces and every writer consumes.
//!
//! Deliberately small. A conversion layer that models everything converts
//! nothing well: the moment the IR grows skeletons, morph targets or material
//! graphs, every writer has to decide how to discard them, and the discards are
//! where silent wrongness lives. What is here is what all four containers can
//! agree on.

/// A single indexed triangle mesh.
///
/// Invariants, checked by [`Mesh::validate`]:
/// - `indices.len()` is a multiple of 3
/// - every index is `< positions.len()`
/// - `normals`/`uvs`, when present, are the same length as `positions`
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// Triangle list. Always indexed in the IR even for formats (STL) that
    /// store a triangle soup — the reader welds, so downstream writers do not
    /// each have to.
    pub indices: Vec<u32>,
    /// Per-vertex normals. `None` when the source did not carry them; writers
    /// that require normals (STL) compute facet normals instead.
    pub normals: Option<Vec<[f32; 3]>>,
    /// Per-vertex texture coordinates.
    pub uvs: Option<Vec<[f32; 2]>>,
    /// Flat base colour, linear RGBA. The one material property every container
    /// can express (or knowingly drop). Textures are out of scope — see the
    /// module docs on `super`.
    pub base_color: Option<[f32; 4]>,
    pub name: Option<String>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Axis-aligned bounds as `(min, max)`. `None` for an empty mesh.
    ///
    /// This is the invariant the conversion matrix checks, because it survives
    /// every lossy step: welding, de-indexing, dropping normals. A conversion
    /// that moved or rescaled geometry would show up here and nowhere else.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let first = *self.positions.first()?;
        let mut min = first;
        let mut max = first;
        for p in &self.positions {
            for a in 0..3 {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
        Some((min, max))
    }

    pub fn validate(&self) -> Result<(), super::MeshError> {
        // NaN and infinity are the one kind of bad geometry that survives every
        // structural check and then poisons everything downstream: they make
        // `bounds()` meaningless, and glTF's REQUIRED position min/max become
        // NaN, which three.js turns into an unrenderable empty scene rather
        // than an error. Reject at the boundary so the failure is attributable
        // to the vendor's bytes instead of surfacing in a viewer later.
        if let Some((i, p)) = self.positions.iter().enumerate().find(|(_, p)| !p.iter().all(|c| c.is_finite())) {
            return Err(super::MeshError::Malformed(format!(
                "position {i} is not finite ({p:?})"
            )));
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err(super::MeshError::Malformed(format!(
                "index count {} is not a multiple of 3",
                self.indices.len()
            )));
        }
        let n = self.positions.len();
        if let Some(bad) = self.indices.iter().find(|i| **i as usize >= n) {
            return Err(super::MeshError::Malformed(format!(
                "index {bad} out of range for {n} positions"
            )));
        }
        if let Some(v) = &self.normals {
            if v.len() != n {
                return Err(super::MeshError::Malformed(format!(
                    "{} normals for {n} positions", v.len()
                )));
            }
        }
        if let Some(v) = &self.uvs {
            if v.len() != n {
                return Err(super::MeshError::Malformed(format!(
                    "{} uvs for {n} positions", v.len()
                )));
            }
        }
        Ok(())
    }

    /// Expand to a triangle soup: one vertex per corner, no shared vertices.
    ///
    /// What STL needs, and what any writer without index support needs.
    pub fn to_soup(&self) -> Vec<[[f32; 3]; 3]> {
        self.indices
            .chunks_exact(3)
            .map(|t| {
                [
                    self.positions[t[0] as usize],
                    self.positions[t[1] as usize],
                    self.positions[t[2] as usize],
                ]
            })
            .collect()
    }

    /// Build an indexed mesh from a triangle soup, merging vertices that are
    /// bit-identical.
    ///
    /// Bit-identity rather than an epsilon: welding by distance silently
    /// changes topology (it closes seams the author meant to keep open), and a
    /// conversion layer has no business making that call. A soup that came from
    /// an indexed mesh via [`Mesh::to_soup`] welds back exactly.
    pub fn from_soup(tris: &[[[f32; 3]; 3]]) -> Self {
        use std::collections::HashMap;
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut indices: Vec<u32> = Vec::with_capacity(tris.len() * 3);
        // f32 has no Hash/Eq, and -0.0 == 0.0 while their bit patterns differ,
        // so normalise the zero sign before keying.
        let key = |p: &[f32; 3]| -> [u32; 3] {
            [
                (if p[0] == 0.0 { 0.0 } else { p[0] }).to_bits(),
                (if p[1] == 0.0 { 0.0 } else { p[1] }).to_bits(),
                (if p[2] == 0.0 { 0.0 } else { p[2] }).to_bits(),
            ]
        };
        let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
        for tri in tris {
            for v in tri {
                let k = key(v);
                let idx = *seen.entry(k).or_insert_with(|| {
                    positions.push(*v);
                    (positions.len() - 1) as u32
                });
                indices.push(idx);
            }
        }
        Mesh { positions, indices, ..Default::default() }
    }

    /// Per-triangle geometric normal, from the winding.
    pub fn facet_normal(tri: &[[f32; 3]; 3]) -> [f32; 3] {
        let [a, b, c] = tri;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 { [n[0] / len, n[1] / len, n[2] / len] } else { [0.0, 0.0, 0.0] }
    }
}
