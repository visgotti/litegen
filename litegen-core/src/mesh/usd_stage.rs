//! USD's data model — layers of specs — and composition of a layer stack into
//! the stage the scene reader walks.
//!
//! # Why composition, and why only this much of it
//!
//! A USD file is not a scene; it is a layer of *opinions*, and the scene is what
//! those opinions compose to. Real exporters lean on that: three.js's
//! `USDZExporter` puts every mesh in its own layer and pulls it in with an
//! external reference, Apple's tools bind one material on an ancestor and expect
//! it to apply below, Omniverse and Maya files carry variant sets and payloads,
//! and `usdcat`/`usdzip` happily keep `over`s and `class`es around. A reader that
//! looks at the root layer's literal `def`s returns a model with parts missing,
//! and nothing says so.
//!
//! So both parsers ([`super::usda`], [`super::usdc`]) produce the same
//! [`Layer`], and [`Stage::compose`] composes them the way Pcp does for the arcs
//! that occur in assets: sublayers, internal and external references, payloads,
//! inherits and specializes, and variant sets (with the authored selection), in
//! LIVRPS strength order, with list-op editing (`prepend`/`append`/`delete`/
//! explicit), relationship-target path translation across arcs, and layer time
//! offsets. What it does not do — relocates, value clips, instancing as a
//! separate prototype graph (instances simply compose like any other prim),
//! per-node ancestral arcs beyond what the namespace walk provides — is either
//! absent from interchange assets or composes to the same geometry this way.
//!
//! # Bounds
//!
//! Composition is where a small file can become a large stage: a reference to a
//! prim that references back, a thousand instances of a thousand instances. Every
//! amplifier is capped — layers loaded, arc depth, sources per prim, prims per
//! stage — and cycles are refused by name rather than looped on. Nothing here
//! recurses on depth the file controls without a hard bound, and the prim walk
//! itself uses an explicit stack.

use std::collections::HashMap;
use std::rc::Rc;

use super::package;
use super::MeshError;

/// Layers one stage may load through sublayers, references and payloads. Real
/// assets use a handful; the bound stops a reference fan-out from turning one
/// small package into thousands of parses.
pub const MAX_LAYERS: usize = 64;
/// How deep arcs may nest (a reference inside a referenced layer inside ...).
const MAX_ARC_DEPTH: usize = 16;
/// Opinions one prim may compose from. Each costs a lookup on every property
/// query, and no real asset stacks more than a few dozen.
const MAX_SOURCES: usize = 256;
/// Composed prims per stage — materials and shaders included, so above the
/// scene's own node ceiling.
pub const MAX_PRIMS: usize = 4 * super::scene::MAX_NODES;
/// Items one list-op may carry. List-op application is quadratic, and nothing
/// legitimate lists thousands of references or relationship targets.
pub const MAX_LIST_OP_ITEMS: usize = 16_384;
/// Longest path string kept. A path is a chain of names; one this long is a
/// hostile chain, not a scene.
pub const MAX_PATH_LEN: usize = 8192;
/// Variants of the default prim's `Animation` set the reader takes for clips
/// ([`Stage::animation_variants`]). Each is one more composition of the whole
/// stage, so a hostile set of thousands would be thousands of them; a real
/// character has a few dozen clips at most.
pub const MAX_ANIMATION_VARIANTS: usize = 64;
/// The variant set Apple's clip layout uses, on the default prim.
pub const ANIMATION_SET: &str = "Animation";

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(format!("usd: {}", detail.into()))
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(format!("usd: {}", detail.into()))
}

// ─── values ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Specifier {
    #[default]
    Def,
    Over,
    Class,
}

/// A USD value as both parsers produce it.
///
/// Arrays of tuples are stored flat (a `point3f[]` of n points is `3n` floats)
/// because that is how the scene wants them and because a `Vec` per element
/// would multiply the memory of a large mesh several times over. Quaternions
/// are always `[x, y, z, w]` here, whatever the source order (USDA text writes
/// `(w, x, y, z)`, the crate stores `x, y, z, w`); matrices are 16 values,
/// row-major, exactly as USD writes them.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    /// `string`.
    Str(String),
    /// `token`.
    Token(String),
    Asset(String),
    Path(String),
    /// Vectors, quaternions, matrices.
    Tuple(Vec<f64>),
    Ints(Vec<i64>),
    /// Arrays of `half`/`float`-based elements, flattened.
    Floats(Vec<f32>),
    /// Arrays of `double`-based elements, flattened.
    Doubles(Vec<f64>),
    /// `string[]`.
    Strs(Vec<String>),
    /// `token[]`.
    Tokens(Vec<String>),
    Assets(Vec<String>),
    Dict(Vec<(String, Value)>),
    /// An untyped list (metadata values whose type the grammar does not name).
    List(Vec<Value>),
    /// `None`: an opinion that there is no value.
    Block,
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => Some(*f as i64),
            Value::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            Value::Float(f) => Some(*f != 0.0),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) | Value::Token(s) | Value::Asset(s) | Value::Path(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_tuple(&self) -> Option<&[f64]> {
        match self {
            Value::Tuple(t) => Some(t),
            _ => None,
        }
    }

    /// Numeric array contents as `f32`, whatever the element precision.
    pub fn floats(&self) -> Option<std::borrow::Cow<'_, [f32]>> {
        match self {
            Value::Floats(v) => Some(std::borrow::Cow::Borrowed(v)),
            Value::Doubles(v) => Some(std::borrow::Cow::Owned(v.iter().map(|d| *d as f32).collect())),
            Value::Ints(v) => Some(std::borrow::Cow::Owned(v.iter().map(|d| *d as f32).collect())),
            _ => None,
        }
    }

    /// Numeric array contents as `f64`.
    pub fn doubles(&self) -> Option<std::borrow::Cow<'_, [f64]>> {
        match self {
            Value::Doubles(v) => Some(std::borrow::Cow::Borrowed(v)),
            Value::Floats(v) => Some(std::borrow::Cow::Owned(v.iter().map(|d| *d as f64).collect())),
            Value::Ints(v) => Some(std::borrow::Cow::Owned(v.iter().map(|d| *d as f64).collect())),
            _ => None,
        }
    }

    pub fn ints(&self) -> Option<&[i64]> {
        match self {
            Value::Ints(v) => Some(v),
            _ => None,
        }
    }

    pub fn strs(&self) -> Option<&[String]> {
        match self {
            Value::Strs(v) | Value::Tokens(v) | Value::Assets(v) => Some(v),
            _ => None,
        }
    }

    pub fn dict(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// A `ListOp`: how one layer edits a list that weaker layers also author.
#[derive(Debug, Clone, PartialEq)]
pub struct ListOp<T> {
    /// An explicit list replaces everything weaker.
    pub explicit: Option<Vec<T>>,
    pub add: Vec<T>,
    pub prepend: Vec<T>,
    pub append: Vec<T>,
    pub delete: Vec<T>,
}

impl<T> Default for ListOp<T> {
    fn default() -> Self {
        ListOp { explicit: None, add: Vec::new(), prepend: Vec::new(), append: Vec::new(), delete: Vec::new() }
    }
}

impl<T: Clone + PartialEq> ListOp<T> {
    pub fn explicit(items: Vec<T>) -> ListOp<T> {
        ListOp { explicit: Some(items), ..Default::default() }
    }

    /// Apply this (stronger) opinion over the list weaker layers composed to,
    /// with Sdf's order of operations: delete, add, prepend, append.
    pub fn apply(&self, weaker: &[T]) -> Vec<T> {
        if let Some(e) = &self.explicit {
            let mut out: Vec<T> = Vec::with_capacity(e.len());
            for x in e {
                if !out.contains(x) {
                    out.push(x.clone());
                }
            }
            return out;
        }
        let mut out: Vec<T> = weaker.iter().filter(|x| !self.delete.contains(x)).cloned().collect();
        for x in &self.add {
            if !out.contains(x) {
                out.push(x.clone());
            }
        }
        if !self.prepend.is_empty() {
            let mut front: Vec<T> = Vec::with_capacity(self.prepend.len() + out.len());
            for x in &self.prepend {
                if !front.contains(x) {
                    front.push(x.clone());
                }
            }
            front.extend(out.into_iter().filter(|x| !self.prepend.contains(x)));
            out = front;
        }
        if !self.append.is_empty() {
            out.retain(|x| !self.append.contains(x));
            for x in &self.append {
                if !out.contains(x) {
                    out.push(x.clone());
                }
            }
        }
        out
    }
}

/// A reference or payload arc.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    /// Empty for an internal reference (a prim in the same layer stack).
    pub asset: String,
    /// Empty to mean the target layer's `defaultPrim`.
    pub prim: String,
    pub offset: f64,
    pub scale: f64,
}

/// An attribute's variability: whether its value may change over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Variability {
    #[default]
    Varying,
    Uniform,
    Config,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttrSpec {
    pub type_name: String,
    /// `uniform`, `config` or (the default) varying, as the statement
    /// declares it. A later statement for the same attribute that names none
    /// leaves it as it was.
    pub variability: Variability,
    pub default: Option<Value>,
    pub samples: Option<Vec<(f64, Value)>>,
    pub connections: Option<ListOp<String>>,
    /// The `interpolation` metadata, kept apart from [`AttrSpec::metadata`].
    pub interpolation: Option<String>,
    /// The `elementSize` metadata, kept apart from [`AttrSpec::metadata`].
    pub element_size: Option<i64>,
    /// Every other metadata key (`documentation`, `customData`,
    /// `displayName`, ...) in the order the layer gives them, under the Sdf
    /// field's name: the text format's `doc` is `documentation`. `interpolation`
    /// and `elementSize` are not repeated here, so a writer re-emits each once.
    pub metadata: Vec<(String, Value)>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RelSpec {
    pub targets: Option<ListOp<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Attr(AttrSpec),
    Rel(RelSpec),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PrimSpec {
    pub specifier: Specifier,
    pub type_name: String,
    /// Child prim names, in authored order.
    pub children: Vec<String>,
    /// Properties in authored order.
    pub props: Vec<(String, Prop)>,
    pub api_schemas: Option<ListOp<String>>,
    pub references: Option<ListOp<Reference>>,
    pub payloads: Option<ListOp<Reference>>,
    pub inherits: Option<ListOp<String>>,
    pub specializes: Option<ListOp<String>>,
    pub variant_set_names: Option<ListOp<String>>,
    pub variant_selection: Vec<(String, String)>,
    /// Variant sets authored on this spec: set name and its variants. The
    /// content of variant `v` of set `s` is the spec at `path{s=v}`.
    pub variant_sets: Vec<(String, Vec<String>)>,
    pub active: Option<bool>,
    pub kind: Option<String>,
    pub custom_data: Option<Vec<(String, Value)>>,
}

impl PrimSpec {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|(n, _)| n == name).map(|(_, p)| p)
    }

    pub fn attr(&self, name: &str) -> Option<&AttrSpec> {
        match self.prop(name) {
            Some(Prop::Attr(a)) => Some(a),
            _ => None,
        }
    }
}

/// One parsed layer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    /// The layer's path inside the package: relative asset paths it authors
    /// resolve against this.
    pub id: String,
    /// Layer metadata (`defaultPrim`, `upAxis`, `metersPerUnit`, ...).
    pub meta: Vec<(String, Value)>,
    /// Sublayer asset paths with their time offset and scale.
    pub sublayers: Vec<(String, f64, f64)>,
    pub root_children: Vec<String>,
    /// Prim specs by path, including variant content (`/A{set=v}`,
    /// `/A{set=v}B`).
    pub specs: HashMap<String, PrimSpec>,
    /// What parsing the layer skipped, for the stage's warnings.
    pub warnings: Vec<String>,
}

impl Layer {
    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.meta.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn meta_f64(&self, key: &str) -> Option<f64> {
        self.meta(key).and_then(Value::as_f64)
    }

    pub fn meta_str(&self, key: &str) -> Option<&str> {
        self.meta(key).and_then(Value::as_str)
    }
}

// ─── paths ──────────────────────────────────────────────────────────────────

/// The path of child `name` under `parent`, in SdfPath syntax: a variant
/// selection binds without a separator (`/A{v=x}B`).
pub fn child_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else if parent.ends_with('}') || name.starts_with('{') {
        format!("{parent}{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// `/A/B.prop` → (`/A/B`, `prop`).
pub fn split_property(path: &str) -> Option<(&str, &str)> {
    // The property separator is the last '.' after the last '/' or '}'.
    let tail_start = path.rfind(['/', '}']).map_or(0, |i| i + 1);
    let dot = path[tail_start..].find('.')? + tail_start;
    Some((&path[..dot], &path[dot + 1..]))
}

/// Resolve a path written relative to `anchor` (a prim path): `../M`,
/// `./Shader.outputs:rgb`, `Child`. Absolute paths pass through. `None` when
/// it climbs above the root.
pub fn absolutize(anchor: &str, path: &str) -> Option<String> {
    if path.starts_with('/') {
        return Some(path.to_string());
    }
    let mut parts: Vec<String> = anchor.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    let mut rest = path;
    loop {
        if let Some(r) = rest.strip_prefix("../") {
            parts.pop()?;
            rest = r;
        } else if rest == ".." {
            parts.pop()?;
            rest = "";
        } else if let Some(r) = rest.strip_prefix("./") {
            rest = r;
        } else if rest == "." {
            rest = "";
        } else {
            break;
        }
    }
    let base = format!("/{}", parts.join("/"));
    if rest.is_empty() {
        return Some(base);
    }
    if let Some(prop) = rest.strip_prefix('.') {
        return Some(format!("{base}.{prop}"));
    }
    Some(child_path(&base, rest))
}

/// Translation of paths from a source layer's namespace to the stage's.
#[derive(Debug, Clone, PartialEq)]
pub struct PathMap {
    /// `(source prefix, stage prefix)`, applied by longest matching prefix.
    rules: Vec<(String, String)>,
}

impl PathMap {
    pub fn identity() -> PathMap {
        PathMap { rules: vec![("/".to_string(), "/".to_string())] }
    }

    fn prefix_matches(prefix: &str, path: &str) -> bool {
        if prefix == "/" {
            return path.starts_with('/');
        }
        path == prefix
            || (path.len() > prefix.len()
                && path.starts_with(prefix)
                && matches!(path.as_bytes()[prefix.len()], b'/' | b'.' | b'{'))
    }

    pub fn apply(&self, path: &str) -> Option<String> {
        let (src, dst) = self
            .rules
            .iter()
            .filter(|(s, _)| PathMap::prefix_matches(s, path))
            .max_by_key(|(s, _)| s.len())?;
        let rest = &path[src.len()..];
        Some(if src == "/" {
            if dst == "/" {
                path.to_string()
            } else {
                format!("{dst}{path}")
            }
        } else if dst == "/" {
            if rest.is_empty() {
                "/".to_string()
            } else {
                // A property of the mapped root (no '/') is meaningless at the
                // pseudo-root.
                format!("/{}", rest.strip_prefix('/')?)
            }
        } else {
            format!("{dst}{rest}")
        })
    }

    /// The map for an arc that brings `target` (in the source's namespace) to
    /// `site` (a stage path). `inherit_outer` keeps the source's own rules for
    /// paths outside the target — right for internal arcs, whose layer stack is
    /// the source's; an external layer's other prims have no stage location.
    fn for_arc(target: &str, site: &str, outer: Option<&PathMap>) -> PathMap {
        let mut rules = vec![(target.to_string(), site.to_string())];
        if let Some(o) = outer {
            rules.extend(o.rules.iter().cloned());
        }
        PathMap { rules }
    }
}

// ─── composition ────────────────────────────────────────────────────────────

/// One opinion a composed prim draws on: a spec in a layer, how its paths map
/// to the stage, and how its times map.
#[derive(Debug, Clone)]
pub struct Source {
    /// Which layer stack the spec's arcs resolve in.
    stack: usize,
    pub layer: usize,
    pub path: String,
    map: Rc<PathMap>,
    /// stage time = offset + scale * layer time.
    pub offset: f64,
    pub scale: f64,
    /// The mapping of the arc this opinion came through, before its layer's
    /// own place in its stack: what an internal arc composes under. Pcp gives
    /// an internal arc its authored offset alone, and the target's layers then
    /// apply their own (usd/pcp/primIndex.cpp#L2251-L2257, OpenUSD v26.11.1).
    node_offset: f64,
    node_scale: f64,
}

/// One prim's composition in progress: the arcs being followed, to cut
/// cycles, and every site (layer stack, path) an arc has already brought into
/// the prim. A site met again is a weaker occurrence of opinions the prim
/// already has, and is skipped: following every path through arcs that
/// rejoin (a lattice of references) was exponential, a 4 KB file six arcs
/// wide taking six times longer per level (2026-10-12 review #17).
#[derive(Default)]
struct Walk {
    chain: Vec<(usize, String)>,
    composed: std::collections::HashSet<(usize, String)>,
}

/// A layer's time codes per second: its own, else its framesPerSecond, else
/// 24, as `SdfLayer::GetTimeCodesPerSecond` falls back
/// (usd/sdf/layer.cpp#L1842-L1860, OpenUSD v26.11.1).
pub fn layer_tcps(l: &Layer) -> f64 {
    let rate = |key: &str| l.meta_f64(key).filter(|v| v.is_finite() && *v > 0.0);
    rate("timeCodesPerSecond").or_else(|| rate("framesPerSecond")).unwrap_or(24.0)
}

#[derive(Debug, Clone)]
struct StackLayer {
    layer: usize,
    offset: f64,
    scale: f64,
}

#[derive(Debug, Clone)]
pub struct Prim {
    pub path: String,
    pub name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub sources: Vec<Source>,
    pub type_name: String,
    pub specifier: Specifier,
    pub active: bool,
}

impl Prim {
    /// Defined (some opinion says `def`), concrete (not a class) and active:
    /// what a renderer draws.
    pub fn is_drawable(&self) -> bool {
        self.specifier == Specifier::Def && self.active
    }
}

/// A composed attribute.
#[derive(Debug, Clone, Default)]
pub struct Attr {
    pub type_name: String,
    pub default: Option<Value>,
    /// Time samples in stage time, sorted.
    pub samples: Vec<(f64, Value)>,
    pub interpolation: Option<String>,
    pub element_size: Option<i64>,
    /// Connection targets, in stage paths.
    pub connections: Vec<String>,
    /// The layer whose opinion supplied the value, for asset resolution.
    pub layer: usize,
}

impl Attr {
    /// The value a viewer shows at the start of the timeline: the earliest
    /// sample when samples decide the value, otherwise the default.
    ///
    /// The strongest opinion with any value decides, and within it samples
    /// beat a default (usd/usd/stage.cpp#L8996-L9069 and #L9383-L9400,
    /// OpenUSD v26.11.1); [`Stage::attr`] keeps samples only from that
    /// opinion. A viewer shows a numeric time: usdrecord renders the stage's
    /// start time code (usdImaging/usdAppUtils/framesArgs.py#L249-L252), and
    /// samples hold their first value before it (usd/sdf/data.cpp#L338-L365).
    /// The default is what `UsdTimeCode::Default()` reads, which nothing here
    /// draws, except as [`timeless`](Self::timeless) for animation the scene
    /// carries.
    pub fn rest(&self) -> Option<&Value> {
        let v = match self.samples.first() {
            Some((_, v)) => v,
            None => self.default.as_ref()?,
        };
        (!matches!(v, Value::Block)).then_some(v)
    }

    /// The value with no animation playing, for an attribute whose animation
    /// the scene carries as a channel (transform ops, joint animation, blend
    /// shape weights): its default, which is what `UsdTimeCode::Default()`
    /// reads (usd/usd/stage.cpp#L7266-L7285), when its samples animate it;
    /// otherwise [`rest`](Self::rest). litegen writes a scene's rest pose that
    /// way, as the default beside the samples. A single sample animates
    /// nothing, carries no channel, and holds at every time, so it wins.
    pub fn timeless(&self) -> Option<&Value> {
        match &self.default {
            Some(d) if self.animated() && !matches!(d, Value::Block) => Some(d),
            _ => self.rest(),
        }
    }

    pub fn animated(&self) -> bool {
        self.samples.len() > 1
    }
}

/// Fetches a file of the package by its normalised path.
pub type Fetch<'a> = dyn FnMut(&str) -> Option<Vec<u8>> + 'a;

/// Parses one layer's bytes. Supplied by `usd.rs`, which knows both formats.
pub type ParseLayer = fn(&[u8], &str) -> Result<Layer, MeshError>;

/// The default prim's variant set `Animation`, when the file follows Apple's
/// clip layout: variants that switch `skel:animationSource`, one clip each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationVariants {
    /// The default prim's path.
    pub prim: String,
    /// The variants, in the order the set lists them.
    pub variants: Vec<String>,
    /// The authored selection, one of `variants`.
    pub selected: String,
}

pub struct Stage {
    /// Shared with the stages [`Stage::recompose_selecting`] makes, so a
    /// recomposition re-parses and copies nothing.
    pub layers: Rc<Vec<Layer>>,
    pub prims: Vec<Prim>,
    pub roots: Vec<usize>,
    pub by_path: HashMap<String, usize>,
    pub warnings: Vec<String>,
    stacks: Vec<Vec<StackLayer>>,
    /// Layer stacks by the package path of their root layer.
    loaded: HashMap<String, usize>,
    /// Package paths being loaded, to refuse sublayer cycles.
    loading: Vec<String>,
    parse: ParseLayer,
    /// Variant selections that beat every authored one: (prim path, set,
    /// variant). Seeds the prim's own composition, as a stronger site's would.
    overrides: Vec<(String, String, String)>,
}

impl Stage {
    /// Compose `root` (and everything it pulls in through `fetch`) into a stage.
    pub fn compose(mut root: Layer, fetch: &mut Fetch<'_>, parse: ParseLayer) -> Result<Stage, MeshError> {
        let root_id = root.id.clone();
        let warnings = std::mem::take(&mut root.warnings);
        let mut stage = Stage {
            layers: Rc::new(vec![root]),
            prims: Vec::new(),
            roots: Vec::new(),
            by_path: HashMap::new(),
            warnings,
            stacks: Vec::new(),
            loaded: HashMap::new(),
            loading: Vec::new(),
            parse,
            overrides: Vec::new(),
        };
        let root_stack = stage.build_stack(0, fetch, parse)?;
        stage.loaded.insert(root_id, root_stack);
        stage.walk(root_stack, fetch, parse)?;
        Ok(stage)
    }

    /// This stage composed again with variant `variant` of set `set` selected
    /// on the prim at `prim`, over whatever the layers author (a stronger
    /// site's selection, as Pcp composes one). The layers are the ones already
    /// loaded: nothing is fetched or parsed, and a layer the first composition
    /// could not fetch is missing again (its warning is in this stage's).
    pub fn recompose_selecting(&self, prim: &str, set: &str, variant: &str) -> Result<Stage, MeshError> {
        let mut overrides = self.overrides.clone();
        overrides.push((prim.to_string(), set.to_string(), variant.to_string()));
        let mut stage = Stage {
            layers: Rc::clone(&self.layers),
            prims: Vec::new(),
            roots: Vec::new(),
            by_path: HashMap::new(),
            warnings: Vec::new(),
            stacks: Vec::new(),
            loaded: HashMap::new(),
            loading: Vec::new(),
            parse: self.parse,
            overrides,
        };
        let mut nothing = |_: &str| -> Option<Vec<u8>> { None };
        let parse = stage.parse;
        let root_id = stage.layers[0].id.clone();
        let root_stack = stage.build_stack(0, &mut nothing, parse)?;
        stage.loaded.insert(root_id, root_stack);
        stage.walk(root_stack, &mut nothing, parse)?;
        Ok(stage)
    }

    /// The default prim's variant set `Animation`, when it follows Apple's
    /// clip layout (a variant that authors `skel:animationSource`) and
    /// selects one of its variants; `None` for any other file, which reads as
    /// a single animation. A set of more than [`MAX_ANIMATION_VARIANTS`] is
    /// refused, naming the cap.
    pub fn animation_variants(&self) -> Result<Option<AnimationVariants>, MeshError> {
        let Some(name) = self.meta("defaultPrim").and_then(Value::as_str) else { return Ok(None) };
        let Some(p) = self.find(&format!("/{name}")) else { return Ok(None) };
        let prim = &self.prims[p];
        let mut variants: Vec<String> = Vec::new();
        let mut listed: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut selected: Option<&str> = None;
        let mut switching: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for s in &prim.sources {
            let Some(spec) = self.spec(s) else { continue };
            if selected.is_none() {
                selected = spec.variant_selection.iter().find(|(k, _)| k == ANIMATION_SET).map(|(_, v)| v.as_str());
            }
            let mut defines = false;
            for (set, vs) in &spec.variant_sets {
                if set == ANIMATION_SET {
                    defines = true;
                    for v in vs {
                        if listed.insert(v.as_str()) {
                            variants.push(v.clone());
                        }
                    }
                }
            }
            if defines {
                // The variants' own content: this path with `{Animation=v}`,
                // and everything below it. One pass over the layer's specs.
                let prefix = format!("{}{{{ANIMATION_SET}=", s.path);
                for (path, spec) in &self.layers[s.layer].specs {
                    if let Some(rest) = path.strip_prefix(prefix.as_str()) {
                        if spec.prop("skel:animationSource").is_some() {
                            if let Some((v, _)) = rest.split_once('}') {
                                switching.insert(v);
                            }
                        }
                    }
                }
            }
        }
        if !variants.iter().any(|v| switching.contains(v.as_str())) {
            return Ok(None);
        }
        if variants.len() > MAX_ANIMATION_VARIANTS {
            return Err(unsupported(format!(
                "the default prim's variant set {ANIMATION_SET} has {} variants; at most {MAX_ANIMATION_VARIANTS} are read",
                variants.len()
            )));
        }
        let Some(selected) = selected.filter(|v| listed.contains(v)) else { return Ok(None) };
        Ok(Some(AnimationVariants { prim: prim.path.clone(), selected: selected.to_string(), variants }))
    }

    fn warn(&mut self, w: String) {
        // Thousands of identical complaints help nobody and cost memory.
        if self.warnings.len() < 200 && !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    /// Load (or reuse) the layer at package path `path` and return its index.
    fn load_layer(&mut self, path: &str, fetch: &mut Fetch<'_>, parse: ParseLayer) -> Result<Option<usize>, MeshError> {
        if let Some(i) = self.layers.iter().position(|l| l.id == path) {
            return Ok(Some(i));
        }
        if self.layers.len() >= MAX_LAYERS {
            return Err(unsupported(format!("the asset pulls in more than {MAX_LAYERS} layers")));
        }
        let Some(bytes) = fetch(path) else { return Ok(None) };
        let mut layer = parse(&bytes, path)?;
        for w in std::mem::take(&mut layer.warnings) {
            self.warn(w);
        }
        Rc::make_mut(&mut self.layers).push(layer);
        Ok(Some(self.layers.len() - 1))
    }

    /// The layer stack rooted at layer `root`: it, then each sublayer and its
    /// own sublayers, strongest first.
    fn build_stack(&mut self, root: usize, fetch: &mut Fetch<'_>, parse: ParseLayer) -> Result<usize, MeshError> {
        let mut entries = vec![StackLayer { layer: root, offset: 0.0, scale: 1.0 }];
        // Explicit stack of (layer index, offset, scale, depth) still to expand.
        let mut todo: Vec<(usize, f64, f64, usize)> = vec![(root, 0.0, 1.0, 0)];
        let mut visiting: Vec<String> = vec![self.layers[root].id.clone()];
        while let Some((li, off, sc, depth)) = todo.pop() {
            let subs = self.layers[li].sublayers.clone();
            let base = self.layers[li].id.clone();
            let mut found = Vec::new();
            for (asset, o, s) in subs {
                let Some(path) = package::resolve_relative(&base, &asset) else {
                    self.warn(format!("sublayer {:?} points outside the package; skipped", short(&asset)));
                    continue;
                };
                if visiting.contains(&path) || self.loading.contains(&path) {
                    self.warn(format!("sublayer {path:?} is part of a cycle; skipped"));
                    continue;
                }
                if depth >= MAX_ARC_DEPTH {
                    return Err(unsupported("sublayers nest too deeply"));
                }
                match self.load_layer(&path, fetch, parse)? {
                    None => self.warn(format!("sublayer {path:?} was not supplied; its opinions are missing")),
                    Some(sub) => {
                        visiting.push(path);
                        // A sublayer's time codes are scaled by the listing
                        // layer's rate over its own, under the listing layer's
                        // mapping (usd/pcp/layerStack.cpp#L1878-L1892).
                        let k = layer_tcps(&self.layers[li]) / layer_tcps(&self.layers[sub]);
                        found.push((sub, off + sc * o, sc * s * k, depth + 1));
                    }
                }
            }
            // Sublayers are listed strongest first; each one's own sublayers
            // come right after it, before its weaker siblings.
            for (sub, o, s, d) in found.iter().rev() {
                todo.push((*sub, *o, *s, *d));
            }
            if let Some(pos) = entries.iter().position(|e| e.layer == li) {
                for (at, (sub, o, s, _)) in (pos + 1..).zip(&found) {
                    entries.insert(at, StackLayer { layer: *sub, offset: *o, scale: *s });
                }
            }
        }
        if entries.len() > MAX_LAYERS {
            return Err(unsupported("the layer stack is too deep"));
        }
        self.stacks.push(entries);
        Ok(self.stacks.len() - 1)
    }

    /// The layer stack for the external layer at `path`, loading it once.
    fn stack_for(&mut self, path: &str, fetch: &mut Fetch<'_>, parse: ParseLayer) -> Result<Option<usize>, MeshError> {
        if let Some(&s) = self.loaded.get(path) {
            return Ok(Some(s));
        }
        if self.loading.iter().any(|p| p == path) {
            return Ok(None);
        }
        self.loading.push(path.to_string());
        let r = match self.load_layer(path, fetch, parse)? {
            None => None,
            Some(li) => Some(self.build_stack(li, fetch, parse)?),
        };
        self.loading.retain(|p| p != path);
        if let Some(s) = r {
            self.loaded.insert(path.to_string(), s);
        }
        Ok(r)
    }

    fn spec(&self, s: &Source) -> Option<&PrimSpec> {
        self.layers.get(s.layer)?.specs.get(&s.path)
    }

    /// Sources for `path` in every layer of stack `stack`.
    fn stack_sources(&self, stack: usize, path: &str, map: &Rc<PathMap>, offset: f64, scale: f64) -> Vec<Source> {
        let mut out = Vec::new();
        for e in &self.stacks[stack] {
            if self.layers[e.layer].specs.contains_key(path) {
                out.push(Source {
                    stack,
                    layer: e.layer,
                    path: path.to_string(),
                    map: map.clone(),
                    offset: offset + scale * e.offset,
                    scale: scale * e.scale,
                    node_offset: offset,
                    node_scale: scale,
                });
            }
        }
        out
    }

    /// The prim-index walk: every prim of the stage, parents first.
    fn walk(&mut self, root_stack: usize, fetch: &mut Fetch<'_>, parse: ParseLayer) -> Result<(), MeshError> {
        let identity = Rc::new(PathMap::identity());
        let mut names: Vec<String> = Vec::new();
        for e in self.stacks[root_stack].clone() {
            for n in &self.layers[e.layer].root_children {
                if !names.contains(n) {
                    names.push(n.clone());
                }
            }
        }
        // (parent prim, name, the opinions inherited from the parent's sources)
        let mut todo: Vec<(Option<usize>, String, Vec<Source>)> = Vec::new();
        for n in names.into_iter().rev() {
            let path = child_path("/", &n);
            let base = self.stack_sources(root_stack, &path, &identity, 0.0, 1.0);
            todo.push((None, n, base));
        }
        while let Some((parent, name, base)) = todo.pop() {
            if self.prims.len() >= MAX_PRIMS {
                return Err(unsupported(format!("the stage composes to more than {MAX_PRIMS} prims")));
            }
            let path = match parent {
                Some(p) => child_path(&self.prims[p].path, &name),
                None => child_path("/", &name),
            };
            if path.len() > MAX_PATH_LEN {
                return Err(malformed("a prim path is longer than any real hierarchy"));
            }
            let mut walk = Walk::default();
            // A selection made for this prim (`recompose_selecting`) is a
            // stronger site's: it beats what the prim's own layers author.
            let seed: Vec<(String, String)> = self.overrides.iter().filter(|(p, _, _)| *p == path).map(|(_, s, v)| (s.clone(), v.clone())).collect();
            let sources = self.expand(base, &path, 0, &seed, &mut walk, fetch, parse)?;
            let index = self.prims.len();
            let mut type_name = String::new();
            let mut specifier = Specifier::Over;
            let mut active = true;
            let mut active_set = false;
            let mut child_names: Vec<String> = Vec::new();
            for s in &sources {
                let Some(spec) = self.spec(s) else { continue };
                if type_name.is_empty() && !spec.type_name.is_empty() {
                    type_name = spec.type_name.clone();
                }
                // The strongest def-or-class decides; overs only add opinions.
                if specifier == Specifier::Over && spec.specifier != Specifier::Over {
                    specifier = spec.specifier;
                }
                if !active_set {
                    if let Some(a) = spec.active {
                        active = a;
                        active_set = true;
                    }
                }
                for c in &spec.children {
                    if !child_names.contains(c) {
                        child_names.push(c.clone());
                    }
                }
            }
            if let Some(p) = parent {
                // A class's subtree is abstract, and so is anything under an
                // undefined prim: neither is drawn.
                if self.prims[p].specifier != Specifier::Def {
                    specifier = self.prims[p].specifier;
                }
                if !self.prims[p].active {
                    active = false;
                }
            }
            let mut children_todo = Vec::with_capacity(child_names.len());
            for c in &child_names {
                let mut child_base = Vec::new();
                for s in &sources {
                    let cp = child_path(&s.path, c);
                    if self.layers[s.layer].specs.contains_key(&cp) {
                        child_base.push(Source { path: cp, ..s.clone() });
                    }
                }
                children_todo.push((Some(index), c.clone(), child_base));
            }
            self.prims.push(Prim { path: path.clone(), name, parent, children: Vec::new(), sources, type_name, specifier, active });
            self.by_path.insert(path, index);
            match parent {
                Some(p) => self.prims[p].children.push(index),
                None => self.roots.push(index),
            }
            for t in children_todo.into_iter().rev() {
                todo.push(t);
            }
        }
        Ok(())
    }

    /// The full, ordered opinion list of a prim whose namespace opinions are
    /// `base`: the arcs those opinions author, expanded in LIVRPS order.
    #[allow(clippy::too_many_arguments)]
    fn expand(
        &mut self,
        base: Vec<Source>,
        site: &str,
        depth: usize,
        outer_selection: &[(String, String)],
        walk: &mut Walk,
        fetch: &mut Fetch<'_>,
        parse: ParseLayer,
    ) -> Result<Vec<Source>, MeshError> {
        if base.is_empty() {
            return Ok(base);
        }
        if depth > MAX_ARC_DEPTH {
            self.warn(format!("composition arcs at {} nest deeper than {MAX_ARC_DEPTH}; the rest were skipped", short(site)));
            return Ok(base);
        }
        // Variant selections: stronger sites first, then this prim's opinions.
        let mut selection: Vec<(String, String)> = outer_selection.to_vec();
        for s in &base {
            if let Some(spec) = self.spec(s) {
                for (set, v) in &spec.variant_selection {
                    if !selection.iter().any(|(k, _)| k == set) {
                        selection.push((set.clone(), v.clone()));
                    }
                }
            }
        }

        // Arcs resolve in the layer stack of the opinion that authored them, so
        // list ops are composed per stack, strongest stack first.
        let mut stacks: Vec<usize> = Vec::new();
        for s in &base {
            if !stacks.contains(&s.stack) {
                stacks.push(s.stack);
            }
        }
        let groups: Vec<Vec<Source>> = stacks.iter().map(|st| base.iter().filter(|s| s.stack == *st).cloned().collect()).collect();
        let class_paths = |this: &Stage, group: &[Source], get: fn(&PrimSpec) -> Option<&ListOp<String>>| -> Vec<String> {
            let mut list: Vec<String> = Vec::new();
            for s in group.iter().rev() {
                if let Some(op) = this.spec(s).and_then(get) {
                    list = op.apply(&list);
                }
            }
            list
        };
        let refs_of = |this: &Stage, group: &[Source], get: fn(&PrimSpec) -> Option<&ListOp<Reference>>| -> Vec<(Reference, Source)> {
            // Each reference remembers the opinion that authored it: its asset
            // path is relative to that layer.
            let mut list: Vec<Reference> = Vec::new();
            let mut owner: Vec<(Reference, Source)> = Vec::new();
            for s in group.iter().rev() {
                if let Some(op) = this.spec(s).and_then(get) {
                    list = op.apply(&list);
                    for r in &list {
                        if !owner.iter().any(|(o, _)| o == r) {
                            owner.push((r.clone(), s.clone()));
                        }
                    }
                }
            }
            list.iter().filter_map(|r| owner.iter().find(|(o, _)| o == r).cloned()).collect()
        };

        // The arcs are followed strongest first (LIVRPS: inherits, variants,
        // references, payloads, specializes), so a site met again later is a
        // weaker occurrence, which `Walk::composed` skips.
        let mut out = base.clone();
        let push = |this: &mut Stage, out: &mut Vec<Source>, sources: Vec<Source>| -> bool {
            for s in sources {
                if out.len() >= MAX_SOURCES {
                    this.warn(format!("{} composes from more than {MAX_SOURCES} opinions; the weakest were ignored", short(site)));
                    return false;
                }
                out.push(s);
            }
            true
        };
        for group in &groups {
            let first = group[0].clone();
            for target in class_paths(self, group, |p| p.inherits.as_ref()) {
                let t = self.internal_arc(&first, (first.node_offset, first.node_scale), &target, site, depth, &selection, walk, fetch, parse)?;
                if !push(self, &mut out, t) {
                    return Ok(out);
                }
            }
        }

        // Variants: for each set any opinion names, the selected variant's
        // content from every opinion that defines it.
        let mut set_names: Vec<String> = Vec::new();
        for s in &base {
            if let Some(spec) = self.spec(s) {
                if let Some(op) = &spec.variant_set_names {
                    for n in op.apply(&[]) {
                        if !set_names.contains(&n) {
                            set_names.push(n);
                        }
                    }
                }
                for (n, _) in &spec.variant_sets {
                    if !set_names.contains(n) {
                        set_names.push(n.clone());
                    }
                }
            }
        }
        for set in set_names {
            let Some(sel) = selection.iter().find(|(k, _)| *k == set).map(|(_, v)| v.clone()) else {
                self.warn(format!(
                    "variant set {set:?} on {} has no selection, so none of its variants applies",
                    short(site)
                ));
                continue;
            };
            if sel.is_empty() {
                continue;
            }
            let mut vbase = Vec::new();
            for s in &base {
                let has = self
                    .spec(s)
                    .is_some_and(|spec| spec.variant_sets.iter().any(|(n, vs)| *n == set && vs.contains(&sel)));
                if has {
                    let vpath = format!("{}{{{}={}}}", s.path, set, sel);
                    if self.layers[s.layer].specs.contains_key(&vpath) {
                        vbase.push(Source { path: vpath, ..s.clone() });
                    }
                }
            }
            if vbase.is_empty() {
                self.warn(format!("variant {set}={sel} selected on {} does not exist; nothing applied", short(site)));
                continue;
            }
            let expanded = self.expand(vbase, site, depth + 1, &selection, walk, fetch, parse)?;
            if !push(self, &mut out, expanded) {
                return Ok(out);
            }
        }

        for (what, get) in [
            ("reference", (|p: &PrimSpec| p.references.as_ref()) as fn(&PrimSpec) -> Option<&ListOp<Reference>>),
            ("payload", |p: &PrimSpec| p.payloads.as_ref()),
        ] {
            for group in &groups {
                for (r, owner) in refs_of(self, group, get) {
                    let t = self.reference_arc(&owner, &r, site, depth, &selection, walk, fetch, parse, what)?;
                    if !push(self, &mut out, t) {
                        return Ok(out);
                    }
                }
            }
        }
        for group in &groups {
            let first = group[0].clone();
            for target in class_paths(self, group, |p| p.specializes.as_ref()) {
                let t = self.internal_arc(&first, (first.node_offset, first.node_scale), &target, site, depth, &selection, walk, fetch, parse)?;
                if !push(self, &mut out, t) {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }

    /// An inherits or specializes arc, or an internal reference: a prim of the
    /// same layer stack.
    #[allow(clippy::too_many_arguments)]
    ///
    /// `time` is the arc's mapping: the owner's node mapping, with an internal
    /// reference's authored offset composed in. The target's layers apply
    /// their own place in the stack on top (`stack_sources`).
    fn internal_arc(
        &mut self,
        owner: &Source,
        time: (f64, f64),
        target: &str,
        site: &str,
        depth: usize,
        selection: &[(String, String)],
        walk: &mut Walk,
        fetch: &mut Fetch<'_>,
        parse: ParseLayer,
    ) -> Result<Vec<Source>, MeshError> {
        let key = (owner.stack, target.to_string());
        if walk.chain.contains(&key) {
            self.warn(format!("a composition cycle through {} was cut at {}", short(target), short(site)));
            return Ok(Vec::new());
        }
        if !walk.composed.insert(key.clone()) {
            return Ok(Vec::new());
        }
        let map = Rc::new(PathMap::for_arc(target, site, Some(&owner.map)));
        let base = self.stack_sources(owner.stack, target, &map, time.0, time.1);
        walk.chain.push(key);
        let r = self.expand(base, site, depth + 1, selection, walk, fetch, parse);
        walk.chain.pop();
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn reference_arc(
        &mut self,
        owner: &Source,
        r: &Reference,
        site: &str,
        depth: usize,
        selection: &[(String, String)],
        walk: &mut Walk,
        fetch: &mut Fetch<'_>,
        parse: ParseLayer,
        what: &str,
    ) -> Result<Vec<Source>, MeshError> {
        if r.asset.is_empty() {
            // The node mapping with the authored offset: no rate factor, and
            // not the authoring layer's own offset (primIndex.cpp#L2251-L2257).
            let time = (owner.node_offset + owner.node_scale * r.offset, owner.node_scale * r.scale);
            let target = if r.prim.is_empty() {
                // An internal reference to "the default prim" of one's own stack.
                let dp = self.layers[self.stacks[owner.stack][0].layer].meta_str("defaultPrim").map(str::to_string);
                let Some(dp) = dp else {
                    self.warn(format!("internal {what} on {} names no prim and the layer has no defaultPrim", short(site)));
                    return Ok(Vec::new());
                };
                child_path("/", &dp)
            } else {
                r.prim.clone()
            };
            return self.internal_arc(owner, time, &target, site, depth, selection, walk, fetch, parse);
        }
        let base_id = self.layers[owner.layer].id.clone();
        let Some(path) = resolve_asset_path(&base_id, &r.asset) else {
            self.warn(format!("{what} {:?} on {} points outside the package; skipped", short(&r.asset), short(site)));
            return Ok(Vec::new());
        };
        if path.ends_with(".usdz") {
            self.warn(format!("{what} {path:?} on {} is a nested package, which is not read; skipped", short(site)));
            return Ok(Vec::new());
        }
        let Some(stack) = self.stack_for(&path, fetch, parse)? else {
            self.warn(format!("{what} {path:?} on {} was not supplied; its content is missing", short(site)));
            return Ok(Vec::new());
        };
        let root_layer = self.stacks[stack][0].layer;
        let target = if r.prim.is_empty() {
            match self.layers[root_layer].meta_str("defaultPrim") {
                Some(dp) => child_path("/", dp),
                None => {
                    self.warn(format!("{what} {path:?} names no prim and that layer has no defaultPrim; skipped"));
                    return Ok(Vec::new());
                }
            }
        } else {
            r.prim.clone()
        };
        let key = (stack, target.clone());
        if walk.chain.contains(&key) {
            self.warn(format!("a composition cycle through {path:?} was cut at {}", short(site)));
            return Ok(Vec::new());
        }
        if !walk.composed.insert(key.clone()) {
            return Ok(Vec::new());
        }
        // The referenced layer's time codes are scaled by the rate of the
        // layer that authors the arc over its own, under that layer's mapping
        // (`owner`): usd/pcp/primIndex.cpp#L2381-L2395, srcLayer per
        // usd/pcp/composeSite.cpp#L128-L132. The stage root's rate was used,
        // which played a reference authored in a 48 tcps layer, to another,
        // twice as fast under a 24 tcps root.
        let ratio = layer_tcps(&self.layers[owner.layer]) / layer_tcps(&self.layers[root_layer]);
        let map = Rc::new(PathMap::for_arc(&target, site, None));
        let offset = owner.offset + owner.scale * r.offset;
        let scale = owner.scale * r.scale * ratio;
        let base = self.stack_sources(stack, &target, &map, offset, scale);
        if base.is_empty() {
            self.warn(format!("{what} target {} in {path:?} does not exist; skipped", short(&target)));
            return Ok(Vec::new());
        }
        walk.chain.push(key);
        let out = self.expand(base, site, depth + 1, selection, walk, fetch, parse);
        walk.chain.pop();
        out
    }

    // ── queries ─────────────────────────────────────────────────────────────

    pub fn find(&self, path: &str) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    /// The composed attribute `name` of prim `p`.
    pub fn attr(&self, p: usize, name: &str) -> Option<Attr> {
        let prim = self.prims.get(p)?;
        let mut out = Attr::default();
        let mut found = false;
        let mut value_decided = false;
        let mut default_decided = false;
        let mut conns: Vec<(&ListOp<String>, &Source)> = Vec::new();
        for s in &prim.sources {
            let Some(a) = self.spec(s).and_then(|spec| spec.attr(name)) else { continue };
            found = true;
            if out.type_name.is_empty() {
                out.type_name = a.type_name.clone();
            }
            if out.interpolation.is_none() {
                out.interpolation = a.interpolation.clone();
            }
            if out.element_size.is_none() {
                out.element_size = a.element_size;
            }
            if !default_decided {
                if let Some(d) = &a.default {
                    out.default = Some(d.clone());
                    default_decided = true;
                    if !value_decided {
                        out.layer = s.layer;
                    }
                }
            }
            if !value_decided {
                if let Some(samples) = &a.samples {
                    out.samples = samples.iter().map(|(t, v)| (s.offset + s.scale * t, v.clone())).collect();
                    out.layer = s.layer;
                    value_decided = true;
                } else if a.default.is_some() {
                    // The strongest opinion is a default: weaker samples are
                    // overridden.
                    value_decided = true;
                }
            }
            if let Some(c) = &a.connections {
                conns.push((c, s));
            }
        }
        if !found {
            return None;
        }
        out.samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        out.samples.dedup_by(|a, b| a.0 == b.0);
        out.connections = self.compose_targets(&conns);
        Some(out)
    }

    /// The composed targets of relationship `name` on prim `p`, as stage paths.
    pub fn rel(&self, p: usize, name: &str) -> Vec<String> {
        let Some(prim) = self.prims.get(p) else { return Vec::new() };
        let mut ops: Vec<(&ListOp<String>, &Source)> = Vec::new();
        for s in &prim.sources {
            if let Some(Prop::Rel(r)) = self.spec(s).and_then(|spec| spec.prop(name)) {
                if let Some(t) = &r.targets {
                    ops.push((t, s));
                }
            }
        }
        self.compose_targets(&ops)
    }

    /// Path list ops composed weakest-first, each target translated through
    /// the map of the opinion that authored it. Targets that have no place in
    /// the stage (outside an external reference's target) are dropped, as Pcp
    /// drops them.
    fn compose_targets(&self, ops: &[(&ListOp<String>, &Source)]) -> Vec<String> {
        let mut list: Vec<String> = Vec::new();
        for (op, s) in ops.iter().rev() {
            let mapped = ListOp {
                explicit: op.explicit.as_ref().map(|v| v.iter().filter_map(|p| s.map.apply(p)).collect()),
                add: op.add.iter().filter_map(|p| s.map.apply(p)).collect(),
                prepend: op.prepend.iter().filter_map(|p| s.map.apply(p)).collect(),
                append: op.append.iter().filter_map(|p| s.map.apply(p)).collect(),
                delete: op.delete.iter().filter_map(|p| s.map.apply(p)).collect(),
            };
            list = mapped.apply(&list);
        }
        list
    }

    /// Property names of prim `p` across every opinion, strongest first.
    pub fn prop_names(&self, p: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        // Seen names in a set: a scan of `out` per name was quadratic in a
        // prim's property count.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if let Some(prim) = self.prims.get(p) {
            for s in &prim.sources {
                if let Some(spec) = self.spec(s) {
                    for (n, _) in &spec.props {
                        if seen.insert(n.as_str()) {
                            out.push(n.clone());
                        }
                    }
                }
            }
        }
        out
    }

    /// A string entry of the prim's strongest `customData`.
    pub fn custom_string(&self, p: usize, key: &str) -> Option<String> {
        let prim = self.prims.get(p)?;
        for s in &prim.sources {
            if let Some(cd) = self.spec(s).and_then(|spec| spec.custom_data.as_ref()) {
                if let Some((_, v)) = cd.iter().find(|(k, _)| k == key) {
                    return v.as_str().map(str::to_string);
                }
            }
        }
        None
    }

    /// The package path of an asset authored in layer `layer`.
    pub fn resolve_asset(&self, layer: usize, asset: &str) -> Option<String> {
        let base = &self.layers.get(layer)?.id;
        resolve_asset_path(base, asset)
    }

    /// Layer metadata of the root layer.
    pub fn meta(&self, key: &str) -> Option<&Value> {
        self.layers[0].meta(key)
    }
}

/// Resolve an asset path authored in the layer at `base` to a package path.
///
/// USD's anchored (`./x`, `../x`) and search-path (`x`) forms both resolve
/// relative to the authoring layer here — the package is the only search path a
/// converter has. Absolute paths, URLs and anything that climbs out of the
/// package are refused, which is the same boundary `package::normalize_relative`
/// draws for every other container.
pub fn resolve_asset_path(base: &str, asset: &str) -> Option<String> {
    let a = asset.trim();
    if a.is_empty() || a.contains("<UDIM>") {
        return None;
    }
    package::resolve_relative(base, a)
}

/// A bounded quote of something from the file, for messages.
pub fn short(s: &str) -> String {
    let t: String = s.chars().take(120).collect();
    if t.len() < s.len() {
        format!("{t}…")
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_ops_follow_sdf_order() {
        let weak = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let op = ListOp {
            prepend: vec!["c".to_string(), "x".to_string()],
            append: vec!["a".to_string()],
            delete: vec!["b".to_string()],
            ..Default::default()
        };
        assert_eq!(op.apply(&weak), vec!["c", "x", "a"]);
        assert_eq!(ListOp::explicit(vec!["z".to_string(), "z".to_string()]).apply(&weak), vec!["z"]);
    }

    #[test]
    fn paths_compose_in_sdf_syntax() {
        assert_eq!(child_path("/", "A"), "/A");
        assert_eq!(child_path("/A", "B"), "/A/B");
        assert_eq!(child_path("/A{v=x}", "B"), "/A{v=x}B");
        assert_eq!(split_property("/A/B.inputs:file"), Some(("/A/B", "inputs:file")));
        assert_eq!(split_property("/A{v=x}B.size"), Some(("/A{v=x}B", "size")));
        assert_eq!(absolutize("/A/B", "../M").as_deref(), Some("/A/M"));
        assert_eq!(absolutize("/A/B", "Sh.outputs:rgb").as_deref(), Some("/A/B/Sh.outputs:rgb"));
        assert_eq!(absolutize("/A", "../../x"), None);
    }

    #[test]
    fn path_maps_translate_by_longest_prefix() {
        let m = PathMap::for_arc("/Geometry", "/Root/Box", None);
        assert_eq!(m.apply("/Geometry/Mesh").as_deref(), Some("/Root/Box/Mesh"));
        assert_eq!(m.apply("/Geometry.size").as_deref(), Some("/Root/Box.size"));
        assert_eq!(m.apply("/Other"), None, "an external layer's other prims have no stage location");
        let inner = PathMap::for_arc("/Proto", "/Inst", Some(&PathMap::identity()));
        assert_eq!(inner.apply("/Materials/M").as_deref(), Some("/Materials/M"));
        assert_eq!(inner.apply("/Proto/Child").as_deref(), Some("/Inst/Child"));
        assert_eq!(inner.apply("/ProtoX").as_deref(), Some("/ProtoX"), "a prefix must end at a path boundary");
    }
}
