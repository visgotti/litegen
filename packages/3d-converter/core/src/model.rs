//! A read model, and writing it: litegen's HTTP converter's pipeline
//! (`litegen-core/src/api/handlers/convert.rs`, `convert_bytes`), split so
//! one read serves any number of writes.

use std::sync::OnceLock;

use litegen_mesh::{normalize, zip, Scene};

use crate::detect::detect_format;
use crate::input::{choose_primary, gather, role_for, roles, unrecognized, Resolver};
use crate::{
    DaeMorphs, Error, ErrorCode, FbxColorSpace, FileRole, Format, Input, InputFileInfo, LightUnits, Loss, ModelInfo, OptionName, Packaging,
    ReadOptions, Unit, UpAxis, UsdzRoot, WriteOptions,
};

/// The scene with its textures transcoded as delivery transcodes them (WebP
/// and KTX2 to PNG; a texture that cannot be decoded, or a KTX2 past the
/// texture budget, left out), and what was left out. `scene` is `None` when
/// that changed nothing, so no second copy is kept.
struct Transcoded {
    scene: Option<Scene>,
    report: normalize::TranscodeReport,
}

/// A model read once, to write as often as needed.
pub struct Model {
    format: Format,
    /// As read, with the read options' frame applied; textures untouched.
    scene: Scene,
    transcoded: OnceLock<Transcoded>,
    /// Measured on the first [`Model::info`] call, never by a read or a
    /// write: the hosted converter never measures.
    info: OnceLock<Result<ModelInfo, Error>>,
    warnings: Vec<String>,
    files: Vec<InputFileInfo>,
}

impl Model {
    /// Read `input`: the hosted converter's `read_scene_with`, then
    /// `normalize::read_frame` when `from_units` or `from_up` is given.
    pub fn read(input: Input<'_>, options: &ReadOptions) -> Result<Model, Error> {
        Model::read_with(input, options, None)
    }

    /// [`Model::read`]. Within [`convert`], `write` is its write options: they
    /// are checked once the format is known and before anything is parsed,
    /// and `fbx_color_space` and `fbx_light_units` are valid when either end
    /// is FBX, `usd_light_units` when either end is USD.
    fn read_with(input: Input<'_>, options: &ReadOptions, write: Option<&WriteOptions>) -> Result<Model, Error> {
        let target = write.map(|w| w.to);
        let gathered = gather(input, options)?;
        let mut entries = gathered.entries;
        let primary = choose_primary(&entries, options, gathered.zip_name.as_deref())?;
        let format = match options.from {
            Some(f) => f,
            None => {
                let e = &entries[primary];
                detect_format(&e.data, e.name.as_deref()).ok_or_else(|| unrecognized(std::slice::from_ref(e), None))?
            }
        };
        if entries[primary].name.is_none() {
            entries[primary].name = Some(format!("model.{}", format.as_str()));
        }
        // An empty file is no format at all, whatever its name says.
        if entries[primary].data.is_empty() {
            return Err(Error::new(ErrorCode::FormatUnrecognized, None).text(format!("{} is empty", entries[primary].name())));
        }
        check_read_options(format, options, target)?;
        if let Some(write) = write {
            check_write_options(write, Some(format))?;
        }
        let resolver = Resolver::new(&entries, primary);
        let read = litegen_mesh::read_scene_with(
            &entries[primary].data,
            format.into(),
            &resolver,
            litegen_mesh::ReadOptions {
                fbx_color_space: options.fbx_color_space.unwrap_or(FbxColorSpace::Srgb).into(),
                fbx_light_units: options.fbx_light_units.unwrap_or(LightUnits::Photometric).into(),
                usd_light_units: options.usd_light_units.unwrap_or(LightUnits::Photometric).into(),
            },
        )
        .map_err(|e| mismatch_names_from(e, format, options.from.is_some()))?;
        let mut scene = read.scene;
        if options.from_units.is_some() || options.from_up.is_some() {
            normalize::read_frame(&mut scene, options.from_units.map_or(1.0, Unit::metres), options.from_up == Some(UpAxis::Z));
        }
        let files = roles(&entries, primary, &resolver.requested());
        Ok(Model { format, scene, transcoded: OnceLock::new(), info: OnceLock::new(), warnings: read.warnings, files })
    }

    /// The format the model was read as.
    pub fn format(&self) -> Format {
        self.format
    }

    /// What the reader measures in it, measured on the first call. Measuring
    /// places every instance of every mesh (mesh's `inspect`), so it fails
    /// with [`ErrorCode::MeshUnsupported`] for a model past mesh's
    /// merged-mesh cap (4,000,000 vertices placed), which still converts.
    pub fn info(&self) -> Result<&ModelInfo, Error> {
        self.info.get_or_init(|| crate::info::of(&self.scene)).as_ref().map_err(Clone::clone)
    }

    /// What reading it dropped or approximated, in prose: a material library
    /// or texture that was not supplied, a glTF extension the converter does
    /// not carry, lines and points, an FBX animation layer.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Every input file and the role it played.
    pub fn files(&self) -> &[InputFileInfo] {
        &self.files
    }

    /// Write the model as `options` says: the hosted converter's
    /// `express_frame` (when `units` or `up` is given), texture transcoding,
    /// `write_scene`, and a zip for [`Packaging::Zip`].
    pub fn write(&self, options: &WriteOptions) -> Result<Conversion, Error> {
        self.write_with(options, None)
    }

    /// [`Model::write`]; `source` is `Some` within [`convert`].
    fn write_with(&self, options: &WriteOptions, source: Option<Format>) -> Result<Conversion, Error> {
        check_write_options(options, source)?;
        let (transcoded, left_out) = self.transcoded();
        if options.units.is_some() || options.up.is_some() {
            let mut framed = transcoded.clone();
            express_frame(&mut framed, options);
            write_scene_as(&framed, left_out, options, &self.warnings)
        } else {
            write_scene_as(transcoded, left_out, options, &self.warnings)
        }
    }

    /// [`convert`]'s write: the model is used up, so its own scene is
    /// transcoded and framed in place, as the hosted converter does, and no
    /// copy of it is made. Its options were checked before the read.
    fn into_conversion(mut self, options: &WriteOptions) -> Result<Conversion, Error> {
        let report = normalize::transcode_textures(&mut self.scene);
        if options.units.is_some() || options.up.is_some() {
            express_frame(&mut self.scene, options);
        }
        write_scene_as(&self.scene, &report, options, &self.warnings)
    }


    /// The scene to write from, transcoded on the first write and cached, and
    /// what transcoding left out.
    fn transcoded(&self) -> (&Scene, &normalize::TranscodeReport) {
        let t = self.transcoded.get_or_init(|| {
            let mut scene = self.scene.clone();
            let report = normalize::transcode_textures(&mut scene);
            let changed = report.changed || !report.dropped.is_empty() || !report.over_budget.is_empty();
            Transcoded { scene: changed.then_some(scene), report }
        });
        (t.scene.as_ref().unwrap_or(&self.scene), &t.report)
    }
}

/// Write `scene` as `options` says: `write_scene`, what transcoding left out
/// as losses, named as the hosted converter names them, and a zip for
/// [`Packaging::Zip`].
fn write_scene_as(
    scene: &Scene,
    left_out: &normalize::TranscodeReport,
    options: &WriteOptions,
    warnings: &[String],
) -> Result<Conversion, Error> {
    let mesh_options = litegen_mesh::WriteOptions {
        self_contained: options.package == Packaging::File,
        fps: options.fps,
        fbx_color_space: options.fbx_color_space.unwrap_or(FbxColorSpace::Srgb).into(),
        fbx_light_units: options.fbx_light_units.unwrap_or(LightUnits::Photometric).into(),
        usd_light_units: options.usd_light_units.unwrap_or(LightUnits::Photometric).into(),
        dae_morphs: options.dae_morphs.unwrap_or(DaeMorphs::Bake).into(),
        usdz_root: options.usdz_root.unwrap_or(UsdzRoot::Usda).into(),
        usd_clips: options.usd_clips.map(Into::into),
    };
    let mut written = litegen_mesh::write_scene(scene, options.to.into(), mesh_options)?;
    for normalize::Undecodable { texture: t, reason: why } in &left_out.dropped {
        let reason = if why.is_empty() { String::new() } else { format!(": {why}") };
        written.losses.push(litegen_mesh::Loss::new(
            litegen_mesh::LossKind::ImageFormats,
            format!("texture {t}"),
            format!("litegen could not decode it to PNG for this container{reason}"),
        ));
    }
    for t in &left_out.over_budget {
        written.losses.push(litegen_mesh::Loss::new(litegen_mesh::LossKind::ImageFormats, format!("texture {t}"), normalize::OVER_BUDGET));
    }
    let files = match options.package {
        Packaging::Zip => vec![OutputFile {
            name: format!("model.{}.zip", options.to.as_str()),
            media_type: "application/zip",
            role: FileRole::Model,
            data: zip::write_package(&written.package)?,
        }],
        Packaging::File | Packaging::Files => written
            .package
            .files
            .into_iter()
            .enumerate()
            .map(|(i, f)| OutputFile {
                media_type: if i == 0 { options.to.media_type() } else { media_type_for(&f.path) },
                role: if i == 0 { FileRole::Model } else { role_for(&f.path) },
                name: f.path,
                data: f.bytes,
            })
            .collect(),
    };
    Ok(Conversion { format: options.to, files, losses: written.losses.into_iter().map(Loss::from).collect(), warnings: warnings.to_vec() })
}

/// The hosted converter's `express_frame` for the write's `units` and `up`.
fn express_frame(scene: &mut Scene, options: &WriteOptions) {
    normalize::express_frame(scene, options.units.map_or(1.0, Unit::metres), options.up == Some(UpAxis::Z));
}

/// A file that is not the format it was read as names `from`, as litegen's
/// HTTP API does: the caller's `from`, or the format taken from the file's
/// name or bytes when there was none.
fn mismatch_names_from(e: litegen_mesh::MeshError, format: Format, from_given: bool) -> Error {
    if !matches!(e, litegen_mesh::MeshError::NotThisFormat { .. }) {
        return Error::from(e);
    }
    let e = Error::new(ErrorCode::FormatMismatch, Some(OptionName::From)).text(e.to_string());
    if from_given {
        e.text("; check that ").option(OptionName::From).text(format!(" ({}) matches the file", format.as_str()))
    } else {
        e.text(format!("; it was taken for {} by its name or bytes: pass ", format.as_str())).option(OptionName::From).text(" to name its format")
    }
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model").field("format", &self.format).field("files", &self.files).finish_non_exhaustive()
    }
}

/// A written model: its files (the model first) and everything the target
/// could not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversion {
    pub format: Format,
    /// One file with [`Packaging::File`] or [`Packaging::Zip`]; the model and
    /// its sidecars with [`Packaging::Files`].
    pub files: Vec<OutputFile>,
    /// What the target could not hold, judged from what the input contains.
    pub losses: Vec<Loss>,
    /// What reading the input dropped or approximated, in prose.
    pub warnings: Vec<String>,
}

/// One written file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// `model.<ext>`, `model.<ext>.zip`, or a sidecar's path (`model.mtl`,
    /// `textures/…`), as the model's references name it.
    pub name: String,
    pub media_type: &'static str,
    pub role: FileRole,
    pub data: Vec<u8>,
}

/// A sidecar's media type, from its extension (litegen's table,
/// `store/mod.rs` `model3d_content_type`).
fn media_type_for(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "mtl" => "model/mtl",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "usda" => "model/vnd.usda",
        _ => "application/octet-stream",
    }
}

/// Read `input` and write it: one call. `fbx_color_space` is one choice for
/// whichever end is FBX, as the hosted converter takes it: the read's or,
/// when it has none, the write's. So is `fbx_light_units`, and
/// `usd_light_units` for whichever end is USD.
pub fn convert(input: Input<'_>, read: &ReadOptions, write: &WriteOptions) -> Result<Conversion, Error> {
    let fbx_color_space = read.fbx_color_space.or(write.fbx_color_space);
    let fbx_light_units = read.fbx_light_units.or(write.fbx_light_units);
    let usd_light_units = read.usd_light_units.or(write.usd_light_units);
    let read = ReadOptions { fbx_color_space, fbx_light_units, usd_light_units, ..read.clone() };
    let write = WriteOptions { fbx_color_space, fbx_light_units, usd_light_units, ..write.clone() };
    let model = Model::read_with(input, &read, Some(&write))?;
    model.into_conversion(&write)
}

/// The format an option of either end belongs to.
#[derive(Clone, Copy)]
struct End {
    /// Whether a format is it.
    is_it: fn(Format) -> bool,
    /// How messages name it.
    formats: &'static str,
}

/// [`End`] of an option of either end; `None` for an option of one side only.
fn end_of(name: OptionName) -> Option<End> {
    match name {
        OptionName::FbxColorSpace | OptionName::FbxLightUnits => Some(End { is_it: |f| f == Format::Fbx, formats: "fbx" }),
        OptionName::UsdLightUnits => {
            Some(End { is_it: |f| matches!(f, Format::Usdz | Format::Usda | Format::Usdc), formats: "usdz, usda or usdc" })
        }
        _ => None,
    }
}

/// The message for an option of either end given where neither end is its
/// format: both ends within [`convert`], `side` alone otherwise.
fn ends_message(e: Error, within_convert: bool, side: OptionName, formats: &str) -> Error {
    if within_convert {
        e.text(" applies when ").option(OptionName::From).text(" or ").option(OptionName::To).text(format!(" is {formats}"))
    } else {
        e.text(" applies when ").option(side).text(format!(" is {formats}"))
    }
}

/// The hosted converter's rules for the read side's options. `target` is the
/// write's format within [`convert`], where an option of either end is also
/// valid when the target is its format.
fn check_read_options(format: Format, options: &ReadOptions, target: Option<Format>) -> Result<(), Error> {
    let allowed = format.info().read_options;
    let given = [
        (options.from_units.is_some(), OptionName::FromUnits),
        (options.from_up.is_some(), OptionName::FromUp),
        (options.fbx_color_space.is_some(), OptionName::FbxColorSpace),
        (options.fbx_light_units.is_some(), OptionName::FbxLightUnits),
        (options.usd_light_units.is_some(), OptionName::UsdLightUnits),
    ];
    for (is_given, name) in given {
        let end = end_of(name);
        let other_end = end.is_some_and(|end| target.is_some_and(end.is_it));
        if is_given && !allowed.contains(&name) && !other_end {
            let e = Error::new(ErrorCode::ParamUnsupported, Some(name)).option(name);
            return Err(match end {
                Some(End { formats, .. }) => ends_message(e, target.is_some(), OptionName::From, formats),
                None => e
                    .text(" applies when ")
                    .option(OptionName::From)
                    .text(format!(" is stl, obj or ply, which carry no units or up axis; {} declares its own", format.as_str())),
            });
        }
    }
    Ok(())
}

/// The hosted converter's rules for the write side's options, and `fps`'s.
/// `source` is the read's format within [`convert`], where an option of
/// either end is also valid when the source is its format.
fn check_write_options(options: &WriteOptions, source: Option<Format>) -> Result<(), Error> {
    let allowed = options.to.info().write_options;
    let given = [
        (options.units.is_some(), OptionName::Units),
        (options.up.is_some(), OptionName::Up),
        (options.fbx_color_space.is_some(), OptionName::FbxColorSpace),
        (options.fbx_light_units.is_some(), OptionName::FbxLightUnits),
        (options.usd_light_units.is_some(), OptionName::UsdLightUnits),
        (options.dae_morphs.is_some(), OptionName::DaeMorphs),
        (options.usdz_root.is_some(), OptionName::UsdzRoot),
        (options.usd_clips.is_some(), OptionName::UsdClips),
        (options.fps.is_some(), OptionName::Fps),
    ];
    for (is_given, name) in given {
        let end = end_of(name);
        let other_end = end.is_some_and(|end| source.is_some_and(end.is_it));
        if is_given && !allowed.contains(&name) && !other_end {
            let e = Error::new(ErrorCode::ParamUnsupported, Some(name)).option(name);
            return Err(match (end, name) {
                (Some(End { formats, .. }), _) => ends_message(e, source.is_some(), OptionName::To, formats),
                (None, OptionName::Fps) => {
                    e.text(" applies when ").option(OptionName::To).text(" is fbx, dae, usdz, usda or usdc, which key animation at a fixed rate")
                }
                (None, OptionName::DaeMorphs) => e.text(" applies when ").option(OptionName::To).text(" is dae"),
                (None, OptionName::UsdzRoot) => e.text(" applies when ").option(OptionName::To).text(" is usdz"),
                (None, OptionName::UsdClips) => e.text(" applies when ").option(OptionName::To).text(" is usda, usdc or usdz"),
                (None, _) => e
                    .text(" applies when ")
                    .option(OptionName::To)
                    .text(format!(" is stl, obj or ply, which carry no units or up axis; {} declares its own", options.to.as_str())),
            });
        }
    }
    if options.fps == Some(0) {
        return Err(Error::new(ErrorCode::ParamInvalid, Some(OptionName::Fps))
            .option(OptionName::Fps)
            .text(" is 0; it takes a whole number of frames per second, 1 or more"));
    }
    Ok(())
}
