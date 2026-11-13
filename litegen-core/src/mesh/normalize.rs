//! Bringing a vendor's mesh to the conventions every delivery meets
//! (PixelGenie 3D doc §2 rule 3, §3D).
//!
//! glTF 2.0's: +Y up, metres, right-handed, the asset's front facing +Z and its
//! right side at −X; textures a core glTF reader decodes (PNG, JPEG); node
//! names a map can key on. Each step reports whether it changed anything, so
//! the caller knows to deliver the vendor's own file beside the result.

use super::math::{self, Mat4};
use super::pose::MAX_RESAMPLED_VALUES;
use super::scene::{AnimPath, Channel, ImageFormat, Interpolation, Node, Scene, Transform, MAX_ANIMATION_KEYS};
use super::MeshError;

/// A signed coordinate axis, as orientations are declared (`"+x"`, `"-z"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    PosX,
    NegX,
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Axis {
    pub fn parse(s: &str) -> Option<Axis> {
        match s.trim().to_ascii_lowercase().as_str() {
            "+x" | "x" => Some(Axis::PosX),
            "-x" => Some(Axis::NegX),
            "+y" | "y" => Some(Axis::PosY),
            "-y" => Some(Axis::NegY),
            "+z" | "z" => Some(Axis::PosZ),
            "-z" => Some(Axis::NegZ),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Axis::PosX => "+x",
            Axis::NegX => "-x",
            Axis::PosY => "+y",
            Axis::NegY => "-y",
            Axis::PosZ => "+z",
            Axis::NegZ => "-z",
        }
    }

    fn vector(self) -> [f64; 3] {
        match self {
            Axis::PosX => [1.0, 0.0, 0.0],
            Axis::NegX => [-1.0, 0.0, 0.0],
            Axis::PosY => [0.0, 1.0, 0.0],
            Axis::NegY => [0.0, -1.0, 0.0],
            Axis::PosZ => [0.0, 0.0, 1.0],
            Axis::NegZ => [0.0, 0.0, -1.0],
        }
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Turn a scene whose vendor builds `up`-up, `front`-facing into +Y up,
/// +Z front, with one new root node, so skins and clips are untouched.
/// `Ok(false)` when it already is.
pub fn orient(scene: &mut Scene, up: Axis, front: Axis) -> Result<bool, MeshError> {
    let (u, f) = (up.vector(), front.vector());
    if u.iter().zip(&f).map(|(a, b)| a * b).sum::<f64>().abs() > 1e-9 {
        return Err(MeshError::Malformed(format!("up {} and front {} are not perpendicular", up.as_str(), front.as_str())));
    }
    if up == Axis::PosY && front == Axis::PosZ {
        return Ok(false);
    }
    // Rows map the vendor's frame onto glTF's: u → +Y, f → +Z, u×f → +X.
    let x = cross(u, f);
    let r: Mat4 = [
        x[0], u[0], f[0], 0.0,
        x[1], u[1], f[1], 0.0,
        x[2], u[2], f[2], 0.0,
        0.0, 0.0, 0.0, 1.0,
    ];
    let children = std::mem::take(&mut scene.roots);
    scene.add_root(Node {
        name: Some("litegen_orientation".into()),
        transform: Transform::from_matrix(r),
        children,
        ..Default::default()
    });
    Ok(true)
}

/// A unit a container with none of its own (STL, OBJ, PLY) may be written in
/// or read as, in metres.
pub fn metres_per_unit(name: &str) -> Option<f64> {
    match name.trim().to_ascii_lowercase().as_str() {
        "m" => Some(1.0),
        "cm" => Some(0.01),
        "mm" => Some(0.001),
        "in" => Some(0.0254),
        "ft" => Some(0.3048),
        _ => None,
    }
}

/// Express a scene in glTF's frame (metres, +Y up, +Z front) as numbers in
/// `unit_m`-metre units, +Z up when `z_up`: one new root scaling by
/// 1/`unit_m` and, for +Z up, turning +90° about +X — +Y to +Z, glTF's front
/// (+Z) to −Y, the turn Blender's glTF importer makes. For a container with
/// no units or axis of its own, where the numbers are all there is. Whether
/// anything changed.
pub fn express_frame(scene: &mut Scene, unit_m: f64, z_up: bool) -> bool {
    wrap_frame(scene, 1.0 / unit_m, if z_up { 1.0 } else { 0.0 })
}

/// [`express_frame`]'s inverse: a file whose numbers are `unit_m`-metre units
/// (+Z up when `z_up`) into glTF's frame.
pub fn read_frame(scene: &mut Scene, unit_m: f64, z_up: bool) -> bool {
    wrap_frame(scene, unit_m, if z_up { -1.0 } else { 0.0 })
}

/// Every root under one new root: uniform `scale`, a quarter turn about +X in
/// the direction of `turn` (+1, −1, or 0 for none).
fn wrap_frame(scene: &mut Scene, scale: f64, turn: f64) -> bool {
    if scale == 1.0 && turn == 0.0 {
        return false;
    }
    let h = std::f64::consts::FRAC_1_SQRT_2;
    let rotation = if turn == 0.0 { math::QUAT_IDENTITY } else { [turn * h, 0.0, 0.0, h] };
    let children = std::mem::take(&mut scene.roots);
    scene.add_root(Node {
        name: Some("litegen_frame".into()),
        transform: Transform::Trs { translation: [0.0; 3], rotation, scale: [scale; 3] },
        children,
        ..Default::default()
    });
    true
}

/// Give every node a distinct name by suffixing repeats `_1`, `_2`, …, so a
/// map from bone to joint name is unambiguous. Whether anything was renamed.
pub fn unique_node_names(scene: &mut Scene) -> bool {
    // Every name a node starts with, so a suffix never takes one a later
    // node already has; and where each name's numbering resumes. Scanning
    // every node per candidate, from `_1`, was cubic in the repeats.
    let original: std::collections::HashSet<String> = scene.nodes.iter().filter_map(|n| n.name.clone()).collect();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut next: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut changed = false;
    for i in 0..scene.nodes.len() {
        let Some(name) = scene.nodes[i].name.clone() else { continue };
        if taken.insert(name.clone()) {
            continue;
        }
        let mut n = next.get(&name).copied().unwrap_or(1);
        let fresh = loop {
            let candidate = format!("{name}_{n}");
            n += 1;
            if !original.contains(&candidate) && !taken.contains(&candidate) {
                break candidate;
            }
        };
        next.insert(name, n);
        taken.insert(fresh.clone());
        scene.nodes[i].name = Some(fresh);
        changed = true;
    }
    changed
}

/// What [`place`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Placement {
    /// Anything moved.
    pub moved: bool,
    /// A height was asked of a mesh with none (flat in Y): it cannot be
    /// scaled to one, and the caller says so rather than ignoring it.
    pub height_skipped: bool,
}

/// Opt-in placement (3D doc §3D) as one new root transform over everything:
/// a uniform scale about the origin to `height_m` tall at rest, then a move
/// putting the lowest point at y = 0 (`ground`) and the bounds' middle on
/// x = z = 0 (`center`). Skins and clips are untouched. The bounds are of
/// everything drawn, points and lines included ([`pose::world_bounds`]), so
/// a point cloud is placed and scaled like a mesh.
///
/// [`pose::world_bounds`]: super::pose::world_bounds
pub fn place(scene: &mut Scene, ground: bool, center: bool, height_m: Option<f64>) -> Result<Placement, MeshError> {
    let rest = super::pose::default_pose(scene);
    let Some((lo, hi)) = super::pose::world_bounds(scene, &rest) else {
        // Nothing to place, unless the scene is too big to pose, which is an
        // error as it always was.
        super::pose::world_primitives(scene, &rest)?;
        return Ok(Placement::default());
    };
    let tall = hi[1] - lo[1];
    let height_skipped = height_m.is_some() && !(tall > 1e-9);
    let scale = match height_m {
        Some(h) if tall > 1e-9 => h / tall,
        _ => 1.0,
    };
    let translation = [
        if center { -scale * (lo[0] + hi[0]) / 2.0 } else { 0.0 },
        if ground { -scale * lo[1] } else { 0.0 },
        if center { -scale * (lo[2] + hi[2]) / 2.0 } else { 0.0 },
    ];
    // Bounds are read in f32: a mesh already placed is left alone.
    let extent = (hi[0] - lo[0]).max(tall).max(hi[2] - lo[2]).max(1.0) * scale;
    if (scale - 1.0).abs() < 1e-5 && translation.iter().all(|t| t.abs() < 1e-5 * extent) {
        return Ok(Placement { moved: false, height_skipped });
    }
    let children = std::mem::take(&mut scene.roots);
    scene.add_root(Node {
        name: Some("litegen_placement".into()),
        transform: Transform::Trs { translation, rotation: [0.0, 0.0, 0.0, 1.0], scale: [scale; 3] },
        children,
        ..Default::default()
    });
    Ok(Placement { moved: true, height_skipped })
}

/// A segmented mesh's parts (3D doc §3F) are its mesh-bearing nodes, each
/// named: the vendor's label, else `part_<n>` for the n-th part in node
/// order. Whether any was named.
pub fn name_parts(scene: &mut Scene) -> bool {
    let mut changed = false;
    for (n, node) in scene.nodes.iter_mut().filter(|node| node.mesh.is_some()).enumerate() {
        if node.name.as_deref().is_none_or(|s| s.trim().is_empty()) {
            node.name = Some(format!("part_{n}"));
            changed = true;
        }
    }
    changed
}

/// Rename nodes by `(from, to)`. Geometry never changes. Whether any did.
pub fn rename_nodes(scene: &mut Scene, renames: &[(String, String)]) -> bool {
    let mut changed = false;
    for node in &mut scene.nodes {
        if let Some((_, to)) = renames.iter().find(|(from, _)| node.name.as_deref() == Some(from.as_str())) {
            if node.name.as_deref() != Some(to.as_str()) {
                node.name = Some(to.clone());
                changed = true;
            }
        }
    }
    changed
}

/// Name the scene's clips `names`, in order. Whether any name changed.
pub fn name_animations(scene: &mut Scene, names: &[String]) -> bool {
    let mut changed = false;
    for (anim, name) in scene.animations.iter_mut().zip(names) {
        if anim.name.as_deref() != Some(name.as_str()) {
            anim.name = Some(name.clone());
            changed = true;
        }
    }
    changed
}

/// Hold the motion joint's horizontal (world X and Z) position where the
/// clip starts, for the whole of clip `anim`, keeping vertical movement —
/// what Mixamo's "In Place" does. The motion joint is the humanoid hips when
/// the rig reads as one ([`inspect::motion_joint`]), whatever sits above it:
/// a static root bone, a scaled or rotated armature (Meshy's 0.01-scale
/// `Armature`), or a non-joint ancestor that carries the motion itself. Its
/// translation is rekeyed at every key of the clip and a grid between
/// ([`inspect::clip_times`]), through its parent's animated world transform —
/// or, for a clip already keyed at a requested rate (`keyed`, from
/// [`resample`]), at its own keys alone, so it stays on that rate. A STEP
/// translation stays STEP. Whether anything changed.
///
/// [`inspect::motion_joint`]: super::inspect::motion_joint
/// [`inspect::clip_times`]: super::inspect::clip_times
pub fn remove_root_motion(scene: &mut Scene, anim: usize, keyed: bool) -> bool {
    let Some(target) = super::inspect::motion_joint(scene) else { return false };
    let times = if keyed {
        let mut own: Vec<f32> = scene.animations[anim].channels.iter().flat_map(|c| c.times.iter().copied()).collect();
        own.sort_by(f32::total_cmp);
        own.dedup();
        own
    } else {
        super::inspect::clip_times(scene, anim)
    };
    let Some(&first) = times.first() else { return false };
    let parent = scene.parents()[target];
    let world_at = |t: f32| super::scene::world_from_locals(scene, &super::pose::sample_pose(scene, anim, t).locals());
    let start = {
        let w = world_at(first);
        [w[target][12], w[target][14]]
    };
    let mut values = Vec::with_capacity(times.len() * 3);
    let mut moved = false;
    for &t in &times {
        let world = world_at(t);
        let at = [world[target][12], world[target][13], world[target][14]];
        moved |= (at[0] - start[0]).abs() > 1e-7 || (at[2] - start[1]).abs() > 1e-7;
        let parent_world = parent.map_or(math::IDENTITY, |p| world[p]);
        let Some(inv) = math::invert(&parent_world) else { return false };
        let local = math::transform_point_f64(&inv, [start[0], at[1], start[1]]);
        values.extend(local.map(|v| v as f32));
    }
    if !moved {
        return false;
    }
    let a = &mut scene.animations[anim];
    let stepped = a.channels.iter().any(|c| c.node == target && c.path == AnimPath::Translation && c.interpolation == Interpolation::Step);
    a.channels.retain(|c| !(c.node == target && c.path == AnimPath::Translation));
    let interpolation = if stepped { Interpolation::Step } else { Interpolation::Linear };
    a.channels.push(super::scene::Channel { node: target, path: AnimPath::Translation, interpolation, times, values });
    true
}

/// The mean of each channel of an image (RGBA, 0..1, as stored: no colour
/// transfer applied), or `None` when it does not decode. What a container
/// that cannot hold a metallic-roughness map keeps of it (`pose::degrade`):
/// the factors are the map's multipliers, so dropping the map alone left
/// glTF's defaults, fully metallic and fully rough.
pub fn channel_means(data: &[u8], format: ImageFormat) -> Option<[f32; 4]> {
    use image::GenericImageView;
    let format = match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Webp => image::ImageFormat::WebP,
        ImageFormat::Ktx2 => return None,
    };
    let img = decode_image(data, format).ok()?;
    let n = u64::from(img.width()) * u64::from(img.height());
    if n == 0 {
        return None;
    }
    // Pixel by pixel, as RGBA: no second, converted copy of the image.
    let mut sums = [0u64; 4];
    for (_, _, px) in img.pixels() {
        for (s, v) in sums.iter_mut().zip(px.0) {
            *s += u64::from(v);
        }
    }
    Some(sums.map(|s| (s as f64 / n as f64 / 255.0) as f32))
}

/// Most bytes one texture may decode to, its output image included as the
/// image crate counts it: 256 MiB, a [`MAX_PACKED_PIXELS`] RGBA map, half the
/// crate's own default. A texture past it is one that does not decode,
/// refused from its header before its decoder allocates.
const MAX_DECODED_IMAGE_BYTES: u64 = 4 * MAX_PACKED_PIXELS;

/// Decode `data` as `format` within [`MAX_DECODED_IMAGE_BYTES`]: every
/// texture this module decodes goes through here.
fn decode_image(data: &[u8], format: image::ImageFormat) -> image::ImageResult<image::DynamicImage> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(data), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
    reader.limits(limits);
    reader.decode()
}

/// One channel of glTF's packed metallic-roughness map: a constant, or one
/// channel of an image read through its texture's scale and bias
/// (UsdUVTexture: `out = in * scale + bias`).
pub enum MrSource<'a> {
    Constant(f32),
    Map { data: &'a [u8], format: ImageFormat, channel: usize, scale: f32, bias: f32 },
}

/// Largest packed map, in pixels (256 MiB of RGBA): two maps of different
/// shapes pack at the larger of each side, which two thin maps could blow up.
/// The one pixel budget of every texture: a KTX2 file's too
/// ([`super::ktx2::MAX_PIXELS`], where it is defined).
const MAX_PACKED_PIXELS: u64 = super::ktx2::MAX_PIXELS;

/// glTF's metallic-roughness map — roughness in G, metallic in B (glTF 2.0
/// §3.9.2) — from two separate sources, as a PNG at the larger map's size,
/// each value baked in so the factors are 1. `None` when a map does not
/// decode. Blender's glTF exporter merges separate images the same way.
pub fn pack_metallic_roughness(metallic: &MrSource, roughness: &MrSource) -> Option<Vec<u8>> {
    let decode = |s: &MrSource| -> Option<Option<image::RgbaImage>> {
        let MrSource::Map { data, format, .. } = s else { return Some(None) };
        let format = match format {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
            ImageFormat::Webp => image::ImageFormat::WebP,
            ImageFormat::Ktx2 => return None,
        };
        Some(Some(decode_image(data, format).ok()?.into_rgba8()))
    };
    let (m, r) = (decode(metallic)?, decode(roughness)?);
    let (w, h) = [&m, &r].into_iter().flatten().fold((1, 1), |(w, h), i| (w.max(i.width()), h.max(i.height())));
    if u64::from(w) * u64::from(h) > MAX_PACKED_PIXELS {
        return None;
    }
    let fit = |i: Option<image::RgbaImage>| {
        i.map(|i| if i.dimensions() == (w, h) { i } else { image::imageops::resize(&i, w, h, image::imageops::FilterType::Triangle) })
    };
    let (m, r) = (fit(m), fit(r));
    let value = |s: &MrSource, img: &Option<image::RgbaImage>, x: u32, y: u32| -> u8 {
        let v = match (s, img) {
            (MrSource::Constant(c), _) => *c,
            (MrSource::Map { channel, scale, bias, .. }, Some(i)) => f32::from(i.get_pixel(x, y).0[(*channel).min(3)]) / 255.0 * scale + bias,
            (MrSource::Map { .. }, None) => 0.0,
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let mut out = image::RgbaImage::new(w, h);
    for (x, y, px) in out.enumerate_pixels_mut() {
        *px = image::Rgba([255, value(roughness, &r, x, y), value(metallic, &m, x, y), 255]);
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(out).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

/// The most encoded image bytes that transcoding may take a scene to
/// ([`transcode_textures`]): half of what a file may be
/// ([`super::MAX_INPUT_BYTES`]), which leaves the other half for geometry, so
/// the file written stays one litegen reads back. A KTX2 texture is far
/// smaller than its PNG, so this is where a scene near the input cap is
/// stopped. Images already PNG, JPEG or WebP are never dropped for it, so a
/// scene whose own images pass it keeps them and loses every KTX2.
pub const TEXTURE_BUDGET: usize = super::MAX_INPUT_BYTES / 2;

/// A texture left out because it could not be decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Undecodable {
    /// The texture, named with its format.
    pub texture: String,
    /// Why: the KTX2 decoder's own words, empty for a broken WebP.
    pub reason: String,
}

/// What [`transcode_textures`] did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TranscodeReport {
    pub changed: bool,
    /// Textures dropped because they could not be decoded — a KTX2 one the
    /// decoder refuses (ZLIB supercompression, a cube map, a broken file), or
    /// a WebP one that is broken — each with the reason it gave.
    pub dropped: Vec<Undecodable>,
    /// KTX2 textures left out because transcoding them would pass the
    /// scene's texture budget ([`TEXTURE_BUDGET`]), each named with its format.
    pub over_budget: Vec<String>,
}

impl TranscodeReport {
    /// What was left out, a sentence for each texture, for a delivery's notes:
    /// first the ones that did not decode, with the decoder's reason where it
    /// gave one, then the ones the budget had no room for.
    pub fn notes(&self) -> Vec<String> {
        let undecodable = self.dropped.iter().map(|Undecodable { texture: t, reason: why }| {
            let reason = if why.is_empty() { String::new() } else { format!(": {why}") };
            format!("texture {t} could not be decoded and was left out{reason}")
        });
        let over = self.over_budget.iter().map(|t| format!("texture {t} was left out: {OVER_BUDGET}"));
        undecodable.chain(over).collect()
    }
}

/// Why a KTX2 texture past [`TEXTURE_BUDGET`] is left out.
pub const OVER_BUDGET: &str = "transcoding it would pass the 64 MiB texture budget";
const _: () = assert!(TEXTURE_BUDGET == 64 << 20, "OVER_BUDGET says 64 MiB");

/// Key every channel of animation `anim` at `fps`: evenly spaced from the
/// clip's start, with its end kept, each value sampled from the source curve.
/// Linear and step channels keep their interpolation; a cubic spline becomes
/// linear through its samples. Whether anything changed: a clip already keyed
/// so is left alone, and so is one [`fits_fps`] declines, before a key is
/// built.
pub fn resample(scene: &mut Scene, anim: usize, fps: u32) -> bool {
    let mut size = animation_size(scene);
    resample_with_size(scene, &mut size, anim, fps).unwrap_or(false)
}

/// [`resample`] in a scene that `size` counts ([`animation_size`], then
/// moved here by each rekey), the budget checked once and in clip `anim`'s
/// channels alone: whether it rekeyed the clip, `size` moved to what the
/// scene then holds; or the first limit it would pass, the scene and `size`
/// untouched. For a caller keying clip after clip, which keeps a running
/// size instead of recounting the scene.
pub fn resample_with_size(scene: &mut Scene, size: &mut RekeyedSize, anim: usize, fps: u32) -> Result<bool, RekeyLimit> {
    let Some((after, times)) = rekey_plan(scene, *size, anim, fps)? else { return Ok(false) };
    let times: Vec<f32> = times.collect();
    let strides: Vec<usize> = scene.animations[anim].channels.iter().map(|c| stride_in(scene, c)).collect();
    for (ch, stride) in scene.animations[anim].channels.iter_mut().zip(strides) {
        let values: Vec<f32> = times.iter().flat_map(|&t| super::pose::sample_channel(ch, t, stride)).collect();
        ch.times = times.clone();
        ch.values = values;
        if ch.interpolation == Interpolation::CubicSpline {
            ch.interpolation = Interpolation::Linear;
        }
    }
    *size = after;
    Ok(true)
}

/// Most intervals [`resample`] keys a clip at: the 30 fps grid's cap.
pub const MAX_RESAMPLE_INTERVALS: usize = 100_000;

/// Whether clip `anim` can be keyed at `fps`: [`check_rekey_budget`] from
/// a fresh [`animation_size`].
pub fn fits_fps(scene: &Scene, anim: usize, fps: u32) -> bool {
    check_rekey_budget(scene, animation_size(scene), anim, fps).is_ok()
}

/// A scene's animation keys and values: as it stands ([`animation_size`]),
/// or once [`resample`] keys one of its clips ([`size_after_rekey`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RekeyedSize {
    /// Keys over every clip, as `Scene::validate` counts them.
    pub keys: usize,
    /// Floats those keys hold: [`Channel::stride`] per key, three times that
    /// for a cubic spline's.
    pub values: usize,
}

/// The keys and values the scene would hold once [`resample`] keys clip
/// `anim` at `fps`: [`size_after_rekey`] from a fresh [`animation_size`].
/// `None` past [`MAX_RESAMPLE_INTERVALS`].
pub fn rekeyed_size(scene: &Scene, anim: usize, fps: u32) -> Option<RekeyedSize> {
    size_after_rekey(scene, animation_size(scene), anim, fps)
}

/// The animation keys and values `scene` holds as it stands, counted as
/// [`RekeyedSize`] counts them.
pub fn animation_size(scene: &Scene) -> RekeyedSize {
    total(scene.animations.iter().flat_map(|a| &a.channels).map(|c| channel_size(scene, c)))
}

/// What a scene holding `now` ([`animation_size`]) would hold once
/// [`resample`] keys clip `anim` at `fps`: `now` with the clip's keys as they
/// stand replaced by its channels on the new grid, linear, counted in the
/// clip's channels alone, so keying clip after clip from a running size
/// costs no recount. `None` past [`MAX_RESAMPLE_INTERVALS`]. `now` for a clip
/// `resample` leaves alone: no such clip, no keys, one instant, `fps` 0, or
/// every channel already on a grid within [`MAX_RESAMPLE_INTERVALS`] (past
/// the cap such a clip is still `None`).
pub fn size_after_rekey(scene: &Scene, now: RekeyedSize, anim: usize, fps: u32) -> Option<RekeyedSize> {
    match regrid(scene, anim, fps) {
        Err(_) => None,
        Ok(None) => Some(now),
        Ok(Some((keys, _))) => Some(size_after(scene, now, anim, keys)),
    }
}

/// A limit keying a clip would take its scene past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RekeyLimit {
    /// More than [`MAX_RESAMPLE_INTERVALS`] intervals: the clip is too long
    /// to key at that rate.
    Grid,
    /// The keys the scene would hold, over [`MAX_ANIMATION_KEYS`].
    Keys(usize),
    /// The values the scene would hold, over [`MAX_RESAMPLED_VALUES`].
    Values(usize),
}

/// Keying clip `anim` at `fps` in a scene holding `now` ([`animation_size`]):
/// the size it leaves ([`size_after_rekey`]), or the first limit it passes,
/// checked in this order: the grid, then [`MAX_ANIMATION_KEYS`] keys, then
/// [`MAX_RESAMPLED_VALUES`] values. A clip [`resample`] leaves alone fits at
/// `now`, whatever `now` holds: no such clip, no keys, one instant, `fps` 0,
/// or every channel already on a grid within [`MAX_RESAMPLE_INTERVALS`]
/// (past the cap such a clip is still [`RekeyLimit::Grid`]).
pub fn check_rekey_budget(scene: &Scene, now: RekeyedSize, anim: usize, fps: u32) -> Result<RekeyedSize, RekeyLimit> {
    Ok(rekey_plan(scene, now, anim, fps)?.map_or(now, |(after, _)| after))
}

/// How [`resample`] keys clip `anim` at `fps` in a scene holding `now`: the
/// size it leaves and the times it keys at; `None` when it leaves the clip
/// alone ([`regrid`]); or the first limit it would pass: the grid, then
/// keys, then values. The one place the budget is decided.
fn rekey_plan(scene: &Scene, now: RekeyedSize, anim: usize, fps: u32) -> Result<Option<(RekeyedSize, impl Iterator<Item = f32>)>, RekeyLimit> {
    let Some((keys, times)) = regrid(scene, anim, fps)? else { return Ok(None) };
    let after = size_after(scene, now, anim, keys);
    if after.keys > MAX_ANIMATION_KEYS {
        Err(RekeyLimit::Keys(after.keys))
    } else if after.values > MAX_RESAMPLED_VALUES {
        Err(RekeyLimit::Values(after.values))
    } else {
        Ok(Some((after, times)))
    }
}

/// The grid [`resample`] keys clip `anim` at `fps` on: keys per channel and
/// their times. `None` when it leaves the clip alone: no such clip, no keys,
/// one instant, `fps` 0, or every channel already on a grid within
/// [`MAX_RESAMPLE_INTERVALS`] (as many keys, each within 1e-4 s, compared
/// lazily, so nothing is built, at a cost of the clip's keys).
/// [`RekeyLimit::Grid`] past [`MAX_RESAMPLE_INTERVALS`], even for a clip
/// already on that grid.
fn regrid(scene: &Scene, anim: usize, fps: u32) -> Result<Option<(usize, impl Iterator<Item = f32>)>, RekeyLimit> {
    let Some((lo, hi)) = rekey_span(scene, anim, fps) else { return Ok(None) };
    let (keys, times) = rekey_times(lo, hi, fps).ok_or(RekeyLimit::Grid)?;
    let on_grid = scene.animations[anim].channels.iter().all(|c| {
        c.times.len() == keys && c.times.iter().zip(times.clone()).all(|(x, y)| (x - y).abs() < 1e-4)
    });
    Ok((!on_grid).then_some((keys, times)))
}

/// `now` with every channel of clip `anim` keyed `grid_keys` times, linear,
/// in place of its own keys. `now` counts the scene, so it holds the clip;
/// a smaller one is the caller's error, caught in debug builds.
fn size_after(scene: &Scene, now: RekeyedSize, anim: usize, grid_keys: usize) -> RekeyedSize {
    let channels = &scene.animations[anim].channels;
    let held = total(channels.iter().map(|c| channel_size(scene, c)));
    let rekeyed = total(channels.iter().map(|c| RekeyedSize { keys: grid_keys, values: grid_keys.saturating_mul(stride_in(scene, c)) }));
    debug_assert!(now.keys >= held.keys && now.values >= held.values, "{now:?} does not count clip {anim}, which holds {held:?}");
    RekeyedSize {
        keys: now.keys.saturating_sub(held.keys).saturating_add(rekeyed.keys),
        values: now.values.saturating_sub(held.values).saturating_add(rekeyed.values),
    }
}

/// Sizes added up, saturating.
fn total(sizes: impl Iterator<Item = RekeyedSize>) -> RekeyedSize {
    sizes.fold(RekeyedSize { keys: 0, values: 0 }, |a, b| RekeyedSize {
        keys: a.keys.saturating_add(b.keys),
        values: a.values.saturating_add(b.values),
    })
}

/// Channel `c`'s keys and values as they stand: [`stride_in`] values per
/// key, three times that per cubic-spline key.
fn channel_size(scene: &Scene, c: &Channel) -> RekeyedSize {
    let stride = stride_in(scene, c);
    let per_key = if c.interpolation == Interpolation::CubicSpline { 3 * stride } else { stride };
    RekeyedSize { keys: c.times.len(), values: c.times.len().saturating_mul(per_key) }
}

/// Floats per key of channel `c` in `scene` ([`Channel::stride`]): its
/// node's morph target count, for weights.
fn stride_in(scene: &Scene, c: &Channel) -> usize {
    c.stride(scene.nodes.get(c.node).and_then(|n| n.mesh).and_then(|m| scene.meshes.get(m)).map_or(0, |m| m.target_count()))
}

/// The span [`resample`] keys clip `anim` over at `fps`: `None` when there
/// is none to key (no such clip, no keys, one instant, `fps` 0).
fn rekey_span(scene: &Scene, anim: usize, fps: u32) -> Option<(f32, f32)> {
    scene.animations.get(anim)?.time_range().filter(|&(lo, hi)| fps > 0 && hi > lo)
}

/// The times [`resample`] keys a clip spanning `lo..hi` at, and how many:
/// `1/fps` apart from `lo`, and `hi` as a key of its own when the last step
/// misses it. `None` past [`MAX_RESAMPLE_INTERVALS`]: never more keys than a
/// writer would resample to (`pose::resample_times` caps its grid the same
/// way). One computation, so what [`size_after_rekey`] counts is what
/// `resample` builds; the times are lazy, so counting builds none.
fn rekey_times(lo: f32, hi: f32, fps: u32) -> Option<(usize, impl Iterator<Item = f32> + Clone)> {
    let step = 1.0 / fps as f64;
    let intervals = (((hi - lo) as f64) / step).floor() as usize;
    if intervals > MAX_RESAMPLE_INTERVALS {
        return None;
    }
    let at = move |k: usize| (lo as f64 + k as f64 * step) as f32;
    let end = ((hi - at(intervals)).abs() > 1e-4).then_some(hi);
    Some((intervals + 1 + usize::from(end.is_some()), (0..=intervals).map(at).chain(end)))
}

/// Remove clips that do not move: no keys, or every key at one instant (a
/// pose, like the bind-pose clip Meshy ships in its rigged file). How many
/// went.
pub fn drop_static_animations(scene: &mut Scene) -> usize {
    let before = scene.animations.len();
    scene.animations.retain(|a| a.time_range().is_some_and(|(lo, hi)| hi - lo > 1e-6));
    before - scene.animations.len()
}

/// Why an image goes from the scene instead of becoming a PNG.
enum LeftOut {
    /// It does not decode: the format named, and the decoder's reason (none
    /// for a WebP).
    Undecodable(&'static str, String),
    /// A KTX2 image the budget has no room for ([`TEXTURE_BUDGET`]).
    OverBudget,
}

/// `data`, a KTX2 texture, as a PNG of the values it stores: no colour
/// conversion, whatever its colour tags say. A texture with no translucent
/// pixel is an RGB PNG, one with some an RGBA one. Or why it cannot be one,
/// in the decoder's words.
fn ktx2_to_png(data: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let decoded = super::ktx2::decode_rgba8(data).map_err(|e| match e {
        super::MeshError::Malformed(why) | super::MeshError::Unsupported(why) => why.strip_prefix("KTX2 texture: ").unwrap_or(&why).to_string(),
        other => other.to_string(),
    })?;
    let (width, height, mut pixels) = (decoded.width, decoded.height, decoded.pixels);
    let opaque = pixels.chunks_exact(4).all(|px| px[3] == 255);
    if opaque {
        // RGBA to RGB in place, the next pixel's colour over this one's alpha.
        for i in 0..pixels.len() / 4 {
            pixels.copy_within(4 * i..4 * i + 3, 3 * i);
        }
        pixels.truncate(pixels.len() / 4 * 3);
    }
    let colour = if opaque { image::ExtendedColorType::Rgb8 } else { image::ExtendedColorType::Rgba8 };
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(&pixels, width, height, colour).map_err(|e| e.to_string())?;
    Ok(png)
}

/// Re-encode every WebP and KTX2 image as PNG: core glTF readers decode only
/// PNG and JPEG, and a delivered file must need no extension to read.
///
/// A KTX2 image (Basis Universal: ETC1S, UASTC, or uncompressed RGBA8, as
/// `ktx2` decodes) becomes a PNG of its stored values. It is one image
/// however many textures sample it. A texture that does not decode (a broken
/// WebP, a KTX2 `ktx2` refuses) is dropped from the materials that use it
/// and reported with the decoder's reason; the vendor's file, delivered
/// beside, keeps it.
///
/// Transcoding never takes the scene's encoded images past [`TEXTURE_BUDGET`]
/// bytes (images already PNG, JPEG or WebP are not dropped for it): a PNG is
/// often several times the KTX2 it replaces, so a scene near the input cap
/// could otherwise become a file litegen cannot read back. The bytes of the
/// PNG and JPEG images already there count first, then each WebP's PNG, then
/// each KTX2 image's, one at a time in image order. From the first KTX2 image
/// whose PNG would pass the budget, every later one is left out, with
/// [`TranscodeReport::over_budget`] naming it.
pub fn transcode_textures(scene: &mut Scene) -> TranscodeReport {
    transcode(scene, true)
}

/// [`transcode_textures`], leaving KTX2 images as they are: the WebP ones
/// still become PNG, and nothing is counted or reported of the KTX2 ones.
/// For a glTF that is to keep its `KHR_texture_basisu` textures.
pub fn transcode_textures_keeping_ktx2(scene: &mut Scene) -> TranscodeReport {
    transcode(scene, false)
}

/// `ktx2`: whether the KTX2 images are transcoded too, or left as they are.
fn transcode(scene: &mut Scene, ktx2: bool) -> TranscodeReport {
    let mut report = TranscodeReport::default();
    let mut dead_images: Vec<(usize, LeftOut)> = Vec::new();
    let mut total: usize = scene.images.iter().filter(|i| matches!(i.format, ImageFormat::Png | ImageFormat::Jpeg)).map(|i| i.data.len()).sum();
    for (i, img) in scene.images.iter_mut().enumerate().filter(|(_, img)| img.format == ImageFormat::Webp) {
        match decode_image(&img.data, image::ImageFormat::WebP) {
            Ok(decoded) => {
                let mut png = Vec::new();
                if decoded.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_ok() {
                    total = total.saturating_add(png.len());
                    img.data = png;
                    img.format = ImageFormat::Png;
                    report.changed = true;
                } else {
                    dead_images.push((i, LeftOut::Undecodable("WebP", String::new())));
                }
            }
            Err(_) => dead_images.push((i, LeftOut::Undecodable("WebP", String::new()))),
        }
    }
    let mut full = total >= TEXTURE_BUDGET;
    for (i, img) in scene.images.iter_mut().enumerate().filter(|(_, img)| ktx2 && img.format == ImageFormat::Ktx2) {
        if full {
            dead_images.push((i, LeftOut::OverBudget));
            continue;
        }
        match ktx2_to_png(&img.data) {
            Ok(png) if total.saturating_add(png.len()) <= TEXTURE_BUDGET => {
                total += png.len();
                img.data = png;
                img.format = ImageFormat::Png;
                report.changed = true;
            }
            Ok(_) => {
                full = true;
                dead_images.push((i, LeftOut::OverBudget));
            }
            Err(why) => dead_images.push((i, LeftOut::Undecodable("KTX2", why))),
        }
    }
    if dead_images.is_empty() {
        return report;
    }
    let dead_textures: Vec<usize> = scene
        .textures
        .iter()
        .enumerate()
        .filter(|(_, t)| dead_images.iter().any(|(i, _)| *i == t.image))
        .map(|(i, _)| i)
        .collect();
    for m in &mut scene.materials {
        for (_, slot) in m.texture_slots_mut() {
            if slot.as_ref().is_some_and(|t| dead_textures.contains(&t.texture)) {
                *slot = None;
                report.changed = true;
            }
        }
    }
    for (i, left_out) in dead_images {
        let name = scene.images[i].name.clone().unwrap_or_else(|| format!("image {i}"));
        match left_out {
            LeftOut::Undecodable(format, reason) => report.dropped.push(Undecodable { texture: format!("{name} ({format})"), reason }),
            LeftOut::OverBudget => report.over_budget.push(format!("{name} (KTX2)")),
        }
    }
    super::pose::prune_unused_textures(scene);
    report
}

/// A PNG's size, whether it has an alpha channel, and its pixels as RGBA8:
/// what the tests of other files read a transcoded texture back with, as only
/// this file may name the `image` crate.
#[cfg(test)]
pub(super) fn png_rgba8(png: &[u8]) -> (u32, u32, bool, Vec<u8>) {
    let decoded = decode_image(png, image::ImageFormat::Png).expect("a PNG");
    (decoded.width(), decoded.height(), decoded.color().has_alpha(), decoded.to_rgba8().into_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixtures;
    use super::super::scene::{Animation, AnimPath, Channel, Interpolation, MorphTarget, Node, Primitive, Scene, SceneMesh};

    /// One triangle near +X, so where +X ends up is visible.
    fn marker() -> Scene {
        let mut s = Scene::default();
        s.meshes.push(SceneMesh {
            primitives: vec![Primitive {
                positions: vec![[1.0, 0.0, 0.0], [1.0, 0.1, 0.0], [1.0, 0.0, 0.1]],
                indices: vec![0, 1, 2],
                ..Default::default()
            }],
            ..Default::default()
        });
        s.nodes.push(Node { name: Some("marker".into()), mesh: Some(0), ..Default::default() });
        s.roots = vec![0];
        s
    }

    fn centroid(scene: &Scene) -> [f32; 3] {
        let m = super::super::pose::flatten(scene).unwrap();
        let (lo, hi) = m.bounds().unwrap();
        [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0]
    }

    #[test]
    fn a_mesh_facing_x_is_turned_to_face_z() {
        let mut s = marker();
        assert!(orient(&mut s, Axis::PosY, Axis::PosX).unwrap());
        let c = centroid(&s);
        assert!(c[2] > 0.9 && c[0].abs() < 0.11, "what faced +X now faces +Z: {c:?}");
    }

    #[test]
    fn a_z_up_mesh_is_stood_up() {
        let mut s = marker();
        // The marker's +X is its front; the vendor's up was +Z.
        assert!(orient(&mut s, Axis::PosZ, Axis::PosX).unwrap());
        let mut probe = marker();
        probe.meshes[0].primitives[0].positions = vec![[0.0, 0.0, 1.0], [0.0, 0.1, 1.0], [0.1, 0.0, 1.0]];
        orient(&mut probe, Axis::PosZ, Axis::PosX).unwrap();
        assert!(centroid(&probe)[1] > 0.9, "what was up (+Z) is now up (+Y)");
        assert!(centroid(&s)[2] > 0.9, "and the front faces +Z");
    }

    #[test]
    fn a_mesh_already_in_conventions_is_left_alone() {
        let mut s = marker();
        let before = s.clone();
        assert!(!orient(&mut s, Axis::PosY, Axis::PosZ).unwrap());
        assert_eq!(s, before);
        assert!(orient(&mut s, Axis::PosY, Axis::PosY).is_err(), "up and front cannot be one axis");
    }

    #[test]
    fn duplicate_node_names_get_numeric_suffixes() {
        let mut s = marker();
        s.nodes.push(Node { name: Some("marker".into()), ..Default::default() });
        s.nodes.push(Node { name: Some("marker".into()), ..Default::default() });
        s.roots = vec![0, 1, 2];
        assert!(unique_node_names(&mut s));
        let names: Vec<_> = s.nodes.iter().map(|n| n.name.clone().unwrap()).collect();
        assert_eq!(names, ["marker", "marker_1", "marker_2"]);
        assert!(!unique_node_names(&mut s), "already unique");
    }

    #[test]
    fn joints_and_clips_are_renamed_without_touching_geometry() {
        let mut s = fixtures::skinned();
        let before = super::super::pose::flatten(&s).unwrap();
        let j0 = s.skins[0].joints[0];
        let old = s.nodes[j0].name.clone().unwrap();
        assert!(rename_nodes(&mut s, &[(old, "hips".into())]));
        assert_eq!(s.nodes[j0].name.as_deref(), Some("hips"));
        assert_eq!(super::super::pose::flatten(&s).unwrap(), before);
        let names: Vec<String> = (0..s.animations.len()).map(|i| format!("clip{i}")).collect();
        assert!(name_animations(&mut s, &names));
        assert_eq!(s.animations[0].name.as_deref(), Some("clip0"));
    }

    #[test]
    fn root_motion_is_removed_horizontally_and_kept_vertically() {
        let mut s = fixtures::skinned();
        let root = s.skins[0].joints[0];
        let (t0, t1) = (0.0f32, 1.0f32);
        s.animations = vec![Animation {
            name: Some("walk".into()),
            channels: vec![Channel {
                node: root,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![t0, t1],
                values: vec![0.0, 0.0, 0.0, 0.3, 0.2, 1.0],
            }],
        }];
        let before = super::super::inspect::inspect(&s).unwrap().animations[0].root_travel_m;
        assert!(before[2] > 0.9);
        assert!(remove_root_motion(&mut s, 0, false));
        let after = super::super::inspect::inspect(&s).unwrap().animations[0].root_travel_m;
        assert!(after[0].abs() < 1e-5 && after[2].abs() < 1e-5, "{after:?}");
        assert!((after[1] - before[1]).abs() < 1e-5, "vertical movement is kept: {after:?} vs {before:?}");
    }

    #[test]
    fn a_clip_is_rekeyed_at_the_asked_rate_and_left_alone_when_it_already_is() {
        let mut s = marker();
        s.animations.push(Animation {
            name: Some("slide".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0, 0.0, 0.0, 0.0, 0.0, 2.0],
            }],
        });
        assert!(resample(&mut s, 0, 4));
        let ch = &s.animations[0].channels[0];
        assert_eq!(ch.times, [0.0, 0.25, 0.5, 0.75, 1.0]);
        assert!((ch.values[3 * 2 + 2] - 1.0).abs() < 1e-6, "halfway, halfway there: {:?}", ch.values);
        assert!(!resample(&mut s, 0, 4), "already keyed at 4 fps");
    }

    /// `nodes` empty nodes, each translated by two keys from 0 to `end`
    /// seconds, in one clip.
    fn translated(nodes: usize, end: f32) -> Scene {
        let mut s = Scene { nodes: vec![Node::default(); nodes], roots: (0..nodes).collect(), ..Default::default() };
        s.animations.push(Animation {
            name: Some("drift".into()),
            channels: (0..nodes)
                .map(|node| Channel { node, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.0, end], values: vec![0.0; 6] })
                .collect(),
        });
        s
    }

    /// `nodes` nodes sharing the marker's triangle, given `targets` morph
    /// targets, each node's weights keyed twice from 0 to `end` seconds, in
    /// one clip.
    fn morphing(nodes: usize, targets: usize, end: f32) -> Scene {
        let mut s = marker();
        s.meshes[0].primitives[0].targets = vec![MorphTarget { positions: vec![[0.0, 0.1, 0.0]; 3], normals: None }; targets];
        s.nodes = vec![Node { mesh: Some(0), ..Default::default() }; nodes];
        s.roots = (0..nodes).collect();
        s.animations.push(Animation {
            name: Some("blink".into()),
            channels: (0..nodes)
                .map(|node| Channel { node, path: AnimPath::Weights, interpolation: Interpolation::Linear, times: vec![0.0, end], values: vec![0.0; 2 * targets] })
                .collect(),
        });
        s
    }

    /// Review leftover f (2026-11-04): what rekeying a clip would leave the
    /// scene holding, counted before a key is built.
    #[test]
    fn rekeyed_size_counts_every_channel() {
        // 90,000 intervals, under the grid cap: 3,000 channels of 90,001 keys,
        // 3 values each.
        let mut s = translated(3_000, 3_000.0);
        assert_eq!(rekeyed_size(&s, 0, 30), Some(RekeyedSize { keys: 270_003_000, values: 810_009_000 }));
        // A second clip, left as it is: 500 keys, a cubic spline's 9 values each.
        s.animations.push(Animation {
            name: Some("idle".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::CubicSpline,
                times: (0..500).map(|k| k as f32).collect(),
                values: vec![0.0; 500 * 9],
            }],
        });
        assert_eq!(rekeyed_size(&s, 0, 30), Some(RekeyedSize { keys: 270_003_500, values: 810_013_500 }));
        assert_eq!(rekeyed_size(&translated(3_000, 3_600.0), 0, 30), None, "108,000 intervals: past the grid cap");

        // An end between two steps is a key of its own, and a cubic spline
        // becomes linear: counted as `resample` builds them.
        let mut s = translated(2, 1.1);
        s.animations[0].channels[1].interpolation = Interpolation::CubicSpline;
        s.animations[0].channels[1].values = vec![0.0; 2 * 9];
        let size = rekeyed_size(&s, 0, 4);
        assert!(resample(&mut s, 0, 4));
        let channels = &s.animations[0].channels;
        assert_eq!(channels[0].times, [0.0, 0.25, 0.5, 0.75, 1.0, 1.1]);
        let built = RekeyedSize { keys: channels.iter().map(|c| c.times.len()).sum(), values: channels.iter().map(|c| c.values.len()).sum() };
        assert_eq!(size, Some(built));
        assert!(!resample(&mut s, 0, 4), "on its 4 fps grid, end key included: left alone");
        assert_eq!(rekeyed_size(&s, 0, 4), Some(animation_size(&s)));
    }

    /// Both budgets are checked before a key is built, so a declined clip
    /// costs nothing, which needs no timing bound to prove.
    #[test]
    fn rekeying_past_the_key_budget_is_declined_without_allocating() {
        // Over both limits; keys are checked first.
        let mut s = translated(3_000, 3_000.0);
        let before = s.clone();
        assert!(!fits_fps(&s, 0, 30), "{:?}", rekeyed_size(&s, 0, 30));
        assert!(!resample(&mut s, 0, 30));
        assert_eq!(s, before);

        // Over the key limit alone: 250 nodes on one single-target mesh.
        let mut s = morphing(250, 1, 3_000.0);
        let before = s.clone();
        assert!(!fits_fps(&s, 0, 30), "{:?}", rekeyed_size(&s, 0, 30));
        assert_eq!(rekeyed_size(&s, 0, 30), Some(RekeyedSize { keys: 22_500_250, values: 22_500_250 }));
        assert!(!resample(&mut s, 0, 30));
        assert_eq!(s, before);
    }

    #[test]
    fn rekeying_past_the_value_budget_is_declined() {
        // One triangle with 1,024 morph targets: 60,001 keys (under 20M) of
        // 1,024 weights each, 61,441,024 values (over 32M).
        let mut s = morphing(1, 1_024, 2_000.0);
        let before = s.clone();
        assert!(!fits_fps(&s, 0, 30), "{:?}", rekeyed_size(&s, 0, 30));
        assert_eq!(rekeyed_size(&s, 0, 30), Some(RekeyedSize { keys: 60_001, values: 61_441_024 }));
        assert!(!resample(&mut s, 0, 30));
        assert_eq!(s, before);
    }

    #[test]
    fn rekeying_within_the_budget_still_rekeys() {
        let mut s = translated(100, 10.0);
        assert!(fits_fps(&s, 0, 30));
        assert_eq!(rekeyed_size(&s, 0, 30), Some(RekeyedSize { keys: 30_100, values: 90_300 }));
        assert!(resample(&mut s, 0, 30));
        assert!(s.animations[0].channels.iter().all(|c| c.times.len() == 301));
    }

    /// What `resample` leaves alone still fits, as before the budget, and
    /// its size is the scene as it stands: no such clip, a clip with no
    /// keys, one held instant, fps 0.
    #[test]
    fn a_clip_with_nothing_to_rekey_still_fits() {
        let mut s = translated(2, 1.0);
        s.animations.push(Animation { name: Some("empty".into()), channels: Vec::new() });
        s.animations.push(Animation {
            name: Some("pose".into()),
            channels: vec![Channel { node: 0, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.5], values: vec![0.0; 3] }],
        });
        for (anim, fps) in [(9, 30), (1, 30), (2, 30), (0, 0)] {
            assert!(fits_fps(&s, anim, fps), "clip {anim} at {fps} fps");
            assert_eq!(rekeyed_size(&s, anim, fps), Some(RekeyedSize { keys: 5, values: 15 }), "clip {anim} at {fps} fps");
            assert!(!resample(&mut s.clone(), anim, fps), "clip {anim} at {fps} fps");
        }
    }

    /// Delivery rekeys clip after clip through `resample_with_size`, which
    /// moves a running size only when it rekeys: after every clip that size
    /// must be a fresh count of the scene.
    #[test]
    fn a_running_size_stays_a_recount_as_clips_are_rekeyed() {
        let mut s = morphing(2, 3, 2.0);
        let held = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        s.animations.push(Animation {
            name: Some("turn".into()),
            channels: vec![
                Channel { node: 0, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.0, 1.1], values: vec![0.0; 6] },
                Channel { node: 1, path: AnimPath::Rotation, interpolation: Interpolation::CubicSpline, times: vec![0.0, 1.1], values: [held, held].concat() },
            ],
        });
        s.animations.push(Animation {
            name: Some("pose".into()),
            channels: vec![Channel { node: 0, path: AnimPath::Scale, interpolation: Interpolation::Linear, times: vec![0.5], values: vec![1.0; 3] }],
        });
        // An end between two steps, a key of its own.
        s.animations.push(Animation {
            name: Some("grow".into()),
            channels: vec![Channel { node: 1, path: AnimPath::Scale, interpolation: Interpolation::Step, times: vec![0.5, 1.55], values: vec![1.0; 6] }],
        });
        // A cubic spline already keyed at 10 fps: left alone, so the size
        // must not move, though its linear count (11 × 4 values) is under
        // what it holds (11 × 12).
        let on_grid: Vec<f32> = (0..=10).map(|k| (k as f64 * 0.1) as f32).collect();
        s.animations.push(Animation {
            name: Some("sway".into()),
            channels: vec![Channel { node: 0, path: AnimPath::Rotation, interpolation: Interpolation::CubicSpline, values: held.repeat(on_grid.len()), times: on_grid }],
        });
        let mut size = animation_size(&s);
        let mut keyed = Vec::new();
        for anim in 0..s.animations.len() {
            keyed.push(resample_with_size(&mut s, &mut size, anim, 10).expect("within the budget"));
            assert_eq!(size, animation_size(&s), "after clip {anim}");
        }
        assert_eq!(keyed, [true, true, false, true, false]);
        assert_eq!(s.animations[1].channels[1].interpolation, Interpolation::Linear, "the cubic spline was rekeyed");
        assert_eq!(s.animations[3].channels[0].times.len(), 12, "0.5 to 1.5 s at 10 fps, and 1.55 s");
        assert_eq!(s.animations[4].channels[0].interpolation, Interpolation::CubicSpline, "the one on the grid was left alone");
    }

    /// What `resample` leaves alone fits whatever the scene holds, an
    /// already over-budget one included: no such clip, no keys, one instant,
    /// fps 0, and every channel already on the grid.
    #[test]
    fn nothing_to_rekey_fits_even_a_scene_over_budget() {
        let mut s = translated(2, 1.0);
        s.animations.push(Animation { name: Some("empty".into()), channels: Vec::new() });
        s.animations.push(Animation {
            name: Some("pose".into()),
            channels: vec![Channel { node: 0, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.5], values: vec![0.0; 3] }],
        });
        s.animations.push(Animation {
            name: Some("keyed".into()),
            channels: vec![Channel { node: 1, path: AnimPath::Translation, interpolation: Interpolation::Linear, times: vec![0.0, 0.25, 0.5, 0.75, 1.0], values: vec![0.0; 15] }],
        });
        let over = RekeyedSize { keys: 0, values: MAX_RESAMPLED_VALUES + 1 };
        for (anim, fps) in [(9, 4), (1, 4), (2, 4), (0, 0), (3, 4)] {
            assert_eq!(check_rekey_budget(&s, over, anim, fps), Ok(over), "clip {anim} at {fps} fps");
        }
        // A clip it would rekey is held to the limit: the scene's 30 values
        // on top of it, clip 0's 12 replaced by 30, is 48 over.
        let counted = animation_size(&s);
        let past = RekeyedSize { values: counted.values + MAX_RESAMPLED_VALUES, ..counted };
        assert_eq!(check_rekey_budget(&s, past, 0, 4), Err(RekeyLimit::Values(MAX_RESAMPLED_VALUES + 48)));
    }

    /// Given a size already past a limit, `resample_with_size` declines
    /// before building anything: the scene and the size stay as they were.
    #[test]
    fn resample_with_size_declines_past_a_limit_and_changes_nothing() {
        let s = translated(1, 1.0);
        let counted = animation_size(&s);
        for (size, limit) in [
            (RekeyedSize { keys: counted.keys + MAX_ANIMATION_KEYS, ..counted }, RekeyLimit::Keys(MAX_ANIMATION_KEYS + 5)),
            (RekeyedSize { values: counted.values + MAX_RESAMPLED_VALUES, ..counted }, RekeyLimit::Values(MAX_RESAMPLED_VALUES + 15)),
        ] {
            let (mut s, mut size) = (s.clone(), size);
            let given = size;
            assert_eq!(resample_with_size(&mut s, &mut size, 0, 4), Err(limit));
            assert_eq!((&s, size), (&translated(1, 1.0), given));
        }
    }

    #[test]
    fn static_clips_are_told_apart_from_motion() {
        let mut s = marker();
        let still = |name: &str, times: Vec<f32>| Animation {
            name: Some(name.into()),
            channels: vec![Channel { node: 0, path: AnimPath::Translation, interpolation: Interpolation::Linear, values: vec![0.0; 3 * times.len()], times }],
        };
        s.animations = vec![still("pose", vec![0.0]), still("walk", vec![0.0, 1.0])];
        assert_eq!(drop_static_animations(&mut s), 1);
        assert_eq!(s.animations.iter().map(|a| a.name.clone().unwrap()).collect::<Vec<_>>(), ["walk"]);
    }

    #[test]
    fn webp_textures_become_png_and_nothing_needs_an_extension() {
        let mut s = fixtures::textured();
        let png = s.images[0].data.clone();
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(img.as_bytes(), img.width(), img.height(), img.color().into())
            .unwrap();
        s.images[0].format = super::super::scene::ImageFormat::Webp;
        s.images[0].data = webp;
        let report = transcode_textures(&mut s);
        assert!(report.changed);
        assert_eq!(s.images[0].format, super::super::scene::ImageFormat::Png);
        assert!(s.images[0].data.starts_with(&[0x89, b'P', b'N', b'G']));
        let glb = super::super::glb::write_scene(&s).unwrap();
        let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let json = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(!json.contains("EXT_texture_webp") && !json.contains("extensionsRequired"), "{json}");
    }

    /// A glTF that keeps its KTX2 textures (`ktx2_textures=keep`) still has
    /// no WebP: that becomes PNG as ever, the KTX2 bytes are untouched, and
    /// the KTX2 image counts nowhere against the PNG budget.
    #[test]
    fn keeping_ktx2_still_transcodes_webp_and_leaves_ktx2_alone() {
        use super::super::scene::{Image, Texture, TextureRef};
        let ktx2 = include_bytes!("testdata/ktx2/Dot_C.ktx2").to_vec();
        let mut s = fixtures::textured();
        let img = image::load_from_memory_with_format(&s.images[0].data, image::ImageFormat::Png).unwrap();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp).encode(img.as_bytes(), img.width(), img.height(), img.color().into()).unwrap();
        s.images[0].format = ImageFormat::Webp;
        s.images[0].data = webp;
        // The scene is already at the budget, so a KTX2 image that was
        // transcoded would be over it.
        s.images.push(Image { name: Some("stored".into()), format: ImageFormat::Png, data: vec![0; TEXTURE_BUDGET] });
        s.images.push(Image { name: Some("basis".into()), format: ImageFormat::Ktx2, data: ktx2.clone() });
        s.textures.push(Texture { name: None, image: 1, sampler: Default::default() });
        s.textures.push(Texture { name: None, image: 2, sampler: Default::default() });
        s.materials[1].emissive_texture = Some(TextureRef { texture: 1, uv_set: 0, transform: None });
        s.materials[1].base_color_texture = Some(TextureRef { texture: 2, uv_set: 0, transform: None });

        let mut full = s.clone();
        let over = transcode_textures(&mut full);
        assert_eq!(over.over_budget, ["basis (KTX2)"], "transcoded too, the KTX2 is over the budget: {over:?}");

        let report = transcode_textures_keeping_ktx2(&mut s);
        assert_eq!(report, TranscodeReport { changed: true, ..Default::default() }, "the WebP changed; the KTX2 is nobody's business");
        assert_eq!(s.images[0].format, ImageFormat::Png, "the WebP is PNG");
        assert_eq!((s.images[2].format, &s.images[2].data), (ImageFormat::Ktx2, &ktx2), "the KTX2 is as it was");
        assert!(s.materials[1].base_color_texture.is_some(), "and still bound");
        s.validate().unwrap();
    }

    /// Transcoding is per image, not per slot: a WebP that only a layer's map
    /// samples becomes a PNG of the same pixels, and the layer keeps its map,
    /// binding and all.
    #[test]
    fn a_webp_layer_texture_is_transcoded_and_its_layer_keeps_it() {
        use super::super::scene::{Image, Sheen, Texture, TextureRef};
        let mut s = fixtures::textured();
        let img = image::load_from_memory_with_format(&s.images[0].data, image::ImageFormat::Png).unwrap();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(img.as_bytes(), img.width(), img.height(), img.color().into())
            .unwrap();
        s.images.push(Image { name: Some("sheen".into()), format: ImageFormat::Webp, data: webp });
        s.textures.push(Texture { name: None, image: s.images.len() - 1, sampler: Default::default() });
        let texture = s.textures.len() - 1;
        let sheen = Sheen { color: [0.9, 0.6, 0.8], color_texture: Some(TextureRef { texture, uv_set: 0, transform: None }), roughness: 0.4, roughness_texture: None };
        s.materials[1].sheen = Some(sheen.clone());
        let report = transcode_textures(&mut s);
        assert!(report.changed && report.dropped.is_empty(), "{:?}", report.dropped);
        assert_eq!(s.materials[1].sheen, Some(sheen), "the sheen keeps its map");
        let png = &s.images[s.textures[texture].image];
        assert_eq!(png.format, ImageFormat::Png);
        let pixels = image::load_from_memory_with_format(&png.data, image::ImageFormat::Png).unwrap();
        assert_eq!(pixels.to_rgba8().into_raw(), img.to_rgba8().into_raw(), "the same pixels");
        s.validate().unwrap();
    }

    /// A 1×1 RGBA8 KTX2 whose header names ZLIB supercompression (scheme 3
    /// of the KTX 2.0 header's `supercompressionScheme`), with its one level
    /// as a stored zlib stream of one red pixel. The KTX2 decoder refuses ZLIB
    /// by name (`KHR_texture_basisu` does not allow it), so it is dropped.
    fn zlib_ktx2() -> Vec<u8> {
        let level = [0x78, 0x01, 0x01, 0x04, 0x00, 0xFB, 0xFF, 0xFF, 0x00, 0x00, 0xFF, 0x04, 0xFF, 0x01, 0xFF];
        let mut b = vec![0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n'];
        // vkFormat R8G8B8A8_UNORM, typeSize, width, height, depth, layers,
        // faces, levels, supercompressionScheme; then no DFD, KVD or SGD.
        for v in [37u32, 1, 1, 1, 0, 0, 1, 1, 3, 0, 0, 0, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&[0; 16]);
        // The level index: offset, length and uncompressed length.
        for v in [b.len() as u64 + 24, level.len() as u64, 4] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&level);
        b
    }

    #[test]
    fn a_ktx2_layer_texture_is_dropped_and_its_layer_kept() {
        // The KTX2's one use is the wedge's sheen colour map: the map goes, the
        // sheen stays with its factors, and the drop is reported.
        use super::super::scene::{Image, Sheen, Texture, TextureRef};
        let mut s = fixtures::textured();
        let ktx2 = zlib_ktx2();
        assert_eq!((ImageFormat::sniff(&ktx2), ktx2.len()), (Some(ImageFormat::Ktx2), 119));
        s.images.push(Image { name: Some("fuzz".into()), format: ImageFormat::Ktx2, data: ktx2 });
        s.textures.push(Texture { name: None, image: s.images.len() - 1, sampler: Default::default() });
        let map = Some(TextureRef { texture: s.textures.len() - 1, uv_set: 0, transform: None });
        let sheen = Sheen { color: [0.9, 0.6, 0.8], color_texture: map, roughness: 0.4, roughness_texture: None };
        s.materials[1].sheen = Some(sheen.clone());
        let report = transcode_textures(&mut s);
        assert_eq!(s.materials[1].sheen, Some(Sheen { color_texture: None, ..sheen }), "the sheen stays, untextured");
        assert!(s.images.iter().all(|i| i.format != ImageFormat::Ktx2), "the KTX2 image is pruned: {:?}", s.images);
        s.validate().unwrap();
        assert!(report.changed);
        // Only the note's stable part: the decoder's reason is `reasons`'.
        assert_eq!(report.dropped.len(), 1, "{:?}", report.dropped);
        assert!(report.dropped[0].texture.starts_with("fuzz (KTX2"), "{:?}", report.dropped);
    }

    #[test]
    fn unnamed_parts_are_named_by_their_place_and_labels_are_kept() {
        let mut scene = super::super::fixtures::cube();
        let mesh = scene.nodes.iter().find_map(|n| n.mesh).unwrap();
        let plain = |name: Option<&str>| super::super::scene::Node {
            name: name.map(String::from),
            transform: super::super::scene::Transform::Trs { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
            children: Vec::new(),
            mesh: Some(mesh),
            skin: None,
            weights: None,
            light: None,
            camera: None,
        };
        scene.nodes = vec![plain(Some("visor")), plain(None), plain(Some("")), plain(None)];
        scene.nodes[3].mesh = None;
        scene.roots = vec![0, 1, 2, 3];
        assert!(name_parts(&mut scene));
        let names: Vec<Option<&str>> = scene.nodes.iter().map(|n| n.name.as_deref()).collect();
        assert_eq!(names, [Some("visor"), Some("part_1"), Some("part_2"), None], "a node without a mesh is no part");
        assert!(!name_parts(&mut scene), "nothing left to name");
    }

    #[test]
    fn placement_scales_grounds_and_centres_through_one_new_root_and_leaves_skins_and_clips_alone() {
        use super::super::scene::Transform;
        let world = |scene: &Scene| super::super::pose::flatten(scene).unwrap().bounds().unwrap();
        let mut scene = super::super::fixtures::cube();
        for r in scene.roots.clone() {
            scene.nodes[r].transform = Transform::Trs { translation: [3.0, 5.0, -1.0], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };
        }
        let nodes = scene.nodes.len();
        assert!(place(&mut scene, true, true, Some(2.0)).unwrap().moved);
        assert_eq!((scene.nodes.len(), scene.roots.len()), (nodes + 1, 1), "one new root over everything");
        let (lo, hi) = world(&scene);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(lo[1], 0.0) && close(hi[1] - lo[1], 2.0), "{lo:?} {hi:?}");
        assert!(close(lo[0] + hi[0], 0.0) && close(lo[2] + hi[2], 0.0), "{lo:?} {hi:?}");
        assert!(!place(&mut scene, true, true, Some(2.0)).unwrap().moved, "already placed: nothing moves");

        let mut rigged = super::super::fixtures::skinned();
        let (skins, clips) = (rigged.skins.clone(), rigged.animations.clone());
        let (lo, hi) = world(&rigged);
        assert!(place(&mut rigged, false, false, Some(f64::from(hi[1] - lo[1]) * 3.0)).unwrap().moved);
        let (nlo, nhi) = world(&rigged);
        assert!(close(nhi[1] - nlo[1], (hi[1] - lo[1]) * 3.0) && close(nlo[1], lo[1] * 3.0), "scaled about the origin");
        assert_eq!((rigged.skins, rigged.animations), (skins, clips), "skins and clips untouched");
    }

    /// Placement is of what is drawn, however it is drawn: a cloud of points
    /// is grounded, centred and scaled to a height as a mesh is, with no
    /// note that it has none, and the points or lines below a model are where
    /// it stands.
    #[test]
    fn a_point_cloud_is_placed() {
        use super::super::pose::{default_pose, world_bounds};
        use super::super::scene::Topology;
        let bounds = |s: &Scene| world_bounds(s, &default_pose(s)).expect("bounds");
        let close = |a: f64, b: f64| (a - b).abs() < 1e-4;

        // The corners of a box 2 wide, 4 high and 2 deep, off to one side.
        let mut cloud = Scene::default();
        let corners: Vec<[f32; 3]> =
            (0..8).map(|i| [10.0 + 2.0 * (i & 1) as f32, 5.0 + 4.0 * ((i >> 1) & 1) as f32, -3.0 + 2.0 * (i >> 2) as f32]).collect();
        cloud.meshes.push(SceneMesh {
            primitives: vec![Primitive { positions: corners, indices: (0..8).collect(), topology: Topology::Points, ..Default::default() }],
            ..Default::default()
        });
        cloud.nodes.push(Node { name: Some("cloud".into()), mesh: Some(0), ..Default::default() });
        cloud.roots = vec![0];
        let placed = place(&mut cloud, true, true, Some(2.0)).unwrap();
        assert_eq!((placed.moved, placed.height_skipped), (true, false));
        let (lo, hi) = bounds(&cloud);
        assert!(close(lo[1], 0.0) && close(hi[1] - lo[1], 2.0), "{lo:?} {hi:?}");
        assert!(close(lo[0] + hi[0], 0.0) && close(lo[2] + hi[2], 0.0), "{lo:?} {hi:?}");
        assert!(close(hi[0] - lo[0], 1.0), "scaled by half, in every axis: {lo:?} {hi:?}");
        assert!(!place(&mut cloud, true, true, Some(2.0)).unwrap().moved, "already placed: nothing moves");

        // `wire`'s grid hangs a metre below its cube, so the model stands on the grid.
        let mut wired = fixtures::wire();
        assert!(place(&mut wired, true, false, None).unwrap().moved);
        let (lo, hi) = bounds(&wired);
        assert!(close(lo[1], 0.0) && close(hi[1], 2.0), "{lo:?} {hi:?}");
    }

    /// N1 (2026-11-01): a texture decodes within a byte budget of litegen's
    /// own, half the image crate's default: a header claiming 10000×10000
    /// RGBA (400 MB) is refused before its decoder allocates for it.
    #[test]
    fn an_oversized_texture_is_refused_before_it_is_decoded() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], body: &[u8]| {
            png.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let mut crc_input = kind.to_vec();
            crc_input.extend_from_slice(body);
            png.extend_from_slice(&crc_input);
            png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&10_000u32.to_be_bytes());
        ihdr.extend_from_slice(&10_000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        // Image data that ends at once: the size alone must refuse it.
        chunk(b"IDAT", &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
        chunk(b"IEND", &[]);
        assert!(matches!(decode_image(&png, image::ImageFormat::Png), Err(image::ImageError::Limits(_))));
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for b in bytes {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
}
