//! What a target container could not hold: mesh's loss report, in the
//! converter's own types.

/// The kind of content a [`Loss`] is about, spelled as mesh spells it
/// (`litegen_mesh::LossKind::as_str`, part of litegen's wire contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LossKind {
    /// Animation clips (the model is delivered at rest).
    Animation,
    /// Skeletons and skin weights (geometry baked into its current pose).
    Skins,
    /// Morph targets (applied at their default weights).
    MorphTargets,
    /// Per-vertex normals.
    Normals,
    /// Texture coordinate sets.
    Uvs,
    /// Per-vertex colour.
    VertexColors,
    /// Texture bindings: a texture that cannot be referenced or applied.
    Textures,
    /// Images in an encoding the target does not accept: a writer drops and
    /// reports a WebP or KTX2 image in every container but glb and gltf,
    /// which carry them through `EXT_texture_webp` and `KHR_texture_basisu`.
    /// A caller that runs `normalize::transcode_textures` first hands the
    /// writer PNG instead, and is left only the images that call could not
    /// decode or left out of its texture budget (its `TranscodeReport`).
    ImageFormats,
    /// Material properties the target's material model cannot express.
    Materials,
    /// Transparency.
    Alpha,
    /// Node hierarchy, names and grouping (transforms are baked).
    Hierarchy,
    /// Separate meshes or primitives merged into one.
    Meshes,
    /// Texture filtering and wrap modes.
    TextureSampling,
    /// Texture transforms that could not be baked into coordinates.
    TextureTransforms,
    /// Geometry the target cannot represent (degenerate triangles, shear).
    Geometry,
    /// Lights the target cannot hold.
    Lights,
    /// Cameras the target cannot hold.
    Cameras,
    /// Point primitives the target cannot hold (left out).
    Points,
    /// Line primitives the target cannot hold (left out).
    Lines,
}

impl LossKind {
    /// Every kind, in mesh's order.
    pub const ALL: [LossKind; 19] = [
        LossKind::Animation,
        LossKind::Skins,
        LossKind::MorphTargets,
        LossKind::Normals,
        LossKind::Uvs,
        LossKind::VertexColors,
        LossKind::Textures,
        LossKind::ImageFormats,
        LossKind::Materials,
        LossKind::Alpha,
        LossKind::Hierarchy,
        LossKind::Meshes,
        LossKind::TextureSampling,
        LossKind::TextureTransforms,
        LossKind::Geometry,
        LossKind::Lights,
        LossKind::Cameras,
        LossKind::Points,
        LossKind::Lines,
    ];

    pub fn as_str(self) -> &'static str {
        litegen_mesh::LossKind::from(self).as_str()
    }
}

impl From<litegen_mesh::LossKind> for LossKind {
    fn from(k: litegen_mesh::LossKind) -> LossKind {
        use litegen_mesh::LossKind as M;
        match k {
            M::Animation => LossKind::Animation,
            M::Skins => LossKind::Skins,
            M::MorphTargets => LossKind::MorphTargets,
            M::Normals => LossKind::Normals,
            M::Uvs => LossKind::Uvs,
            M::VertexColors => LossKind::VertexColors,
            M::Textures => LossKind::Textures,
            M::ImageFormats => LossKind::ImageFormats,
            M::Materials => LossKind::Materials,
            M::Alpha => LossKind::Alpha,
            M::Hierarchy => LossKind::Hierarchy,
            M::Meshes => LossKind::Meshes,
            M::TextureSampling => LossKind::TextureSampling,
            M::TextureTransforms => LossKind::TextureTransforms,
            M::Geometry => LossKind::Geometry,
            M::Lights => LossKind::Lights,
            M::Cameras => LossKind::Cameras,
            M::Points => LossKind::Points,
            M::Lines => LossKind::Lines,
        }
    }
}

impl From<LossKind> for litegen_mesh::LossKind {
    fn from(k: LossKind) -> litegen_mesh::LossKind {
        use litegen_mesh::LossKind as M;
        match k {
            LossKind::Animation => M::Animation,
            LossKind::Skins => M::Skins,
            LossKind::MorphTargets => M::MorphTargets,
            LossKind::Normals => M::Normals,
            LossKind::Uvs => M::Uvs,
            LossKind::VertexColors => M::VertexColors,
            LossKind::Textures => M::Textures,
            LossKind::ImageFormats => M::ImageFormats,
            LossKind::Materials => M::Materials,
            LossKind::Alpha => M::Alpha,
            LossKind::Hierarchy => M::Hierarchy,
            LossKind::Meshes => M::Meshes,
            LossKind::TextureSampling => M::TextureSampling,
            LossKind::TextureTransforms => M::TextureTransforms,
            LossKind::Geometry => M::Geometry,
            LossKind::Lights => M::Lights,
            LossKind::Cameras => M::Cameras,
            LossKind::Points => M::Points,
            LossKind::Lines => M::Lines,
        }
    }
}

/// One thing the target could not hold: its kind, what it was, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loss {
    pub kind: LossKind,
    /// What was dropped or approximated, e.g. `animation "walk"`.
    pub what: String,
    /// Why the target could not hold it.
    pub why: String,
}

impl From<litegen_mesh::Loss> for Loss {
    fn from(l: litegen_mesh::Loss) -> Loss {
        let litegen_mesh::Loss { kind, what, why } = l;
        Loss { kind: kind.into(), what, why }
    }
}
