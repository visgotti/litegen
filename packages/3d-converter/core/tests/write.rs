//! Writing: the same bytes and losses as litegen's HTTP converter, every
//! packaging, each file's role and media type, the write options' rules and
//! their effect on the file.

use litegen_3d_converter::{
    convert, Conversion, DaeMorphs, ErrorCode, FbxColorSpace, FileRole, Format, Input, LightUnits, LossKind, Model,
    OptionName, Packaging, ReadOptions, Unit, UpAxis, UsdClips, UsdzRoot, WriteOptions,
};
use litegen_mesh::{fixtures, normalize, package::Package, scene::ImageFormat, write_scene, zip, MeshFormat, Scene};

fn mesh(f: Format) -> MeshFormat {
    MeshFormat::parse(f.as_str()).unwrap()
}

fn glb_of(scene: &Scene) -> Vec<u8> {
    write_scene(scene, MeshFormat::Glb, litegen_mesh::WriteOptions::default()).unwrap().package.files.remove(0).bytes
}

/// [`hosted_lit`] with both light units at their default, `photometric`, and
/// the write's own choices (`dae_morphs`, `usdz_root`, `usd_clips`) at theirs.
fn hosted(
    bytes: &[u8],
    from: MeshFormat,
    to: MeshFormat,
    read_as: Option<(f64, bool)>,
    write_as: Option<(f64, bool)>,
    fbx_colors: litegen_mesh::FbxColorSpace,
    zipped: bool,
) -> litegen_mesh::SceneWrite {
    use litegen_mesh::LightUnits::Photometric;
    hosted_lit(bytes, from, to, read_as, write_as, fbx_colors, zipped, (Photometric, Photometric), litegen_mesh::WriteOptions::default())
}

/// litegen-core/src/api/handlers/convert.rs `convert_bytes`, call for call:
/// what the hosted converter answers for the same input and options, with
/// `ktx2_textures` at its default, `png` (the package has no `keep`),
/// `(fbx_light_units, usd_light_units)` given to both the read and the write,
/// and the write's own choices (`dae_morphs`, `usdz_root`, `usd_clips`) taken
/// from `choices`.
#[allow(clippy::too_many_arguments)]
fn hosted_lit(
    bytes: &[u8],
    from: MeshFormat,
    to: MeshFormat,
    read_as: Option<(f64, bool)>,
    write_as: Option<(f64, bool)>,
    fbx_colors: litegen_mesh::FbxColorSpace,
    zipped: bool,
    (fbx_light_units, usd_light_units): (litegen_mesh::LightUnits, litegen_mesh::LightUnits),
    choices: litegen_mesh::WriteOptions,
) -> litegen_mesh::SceneWrite {
    let read_options = litegen_mesh::ReadOptions { fbx_color_space: fbx_colors, fbx_light_units, usd_light_units };
    let read = litegen_mesh::read_scene_with(bytes, from, &litegen_mesh::NoSidecars, read_options).unwrap();
    let mut scene = read.scene;
    if let Some((unit_m, z_up)) = read_as {
        normalize::read_frame(&mut scene, unit_m, z_up);
    }
    if let Some((unit_m, z_up)) = write_as {
        normalize::express_frame(&mut scene, unit_m, z_up);
    }
    let transcoded = normalize::transcode_textures(&mut scene);
    let write_options = litegen_mesh::WriteOptions {
        self_contained: !zipped,
        fbx_color_space: fbx_colors,
        fbx_light_units,
        usd_light_units,
        dae_morphs: choices.dae_morphs,
        usdz_root: choices.usdz_root,
        usd_clips: choices.usd_clips,
        ..Default::default()
    };
    let mut written = write_scene(&scene, to, write_options).unwrap();
    for normalize::Undecodable { texture: t, reason: why } in &transcoded.dropped {
        let reason = if why.is_empty() { String::new() } else { format!(": {why}") };
        written.losses.push(litegen_mesh::Loss::new(
            litegen_mesh::LossKind::ImageFormats,
            format!("texture {t}"),
            format!("litegen could not decode it to PNG for this container{reason}"),
        ));
    }
    for t in &transcoded.over_budget {
        written.losses.push(litegen_mesh::Loss::new(litegen_mesh::LossKind::ImageFormats, format!("texture {t}"), normalize::OVER_BUDGET));
    }
    if zipped {
        written.package = Package::single(format!("model.{to}.zip"), zip::write_package(&written.package).unwrap());
    }
    written
}

fn assert_same(got: &litegen_3d_converter::Conversion, want: &litegen_mesh::SceneWrite, what: &str) {
    let got_files: Vec<(&str, &[u8])> = got.files.iter().map(|f| (f.name.as_str(), f.data.as_slice())).collect();
    let want_files: Vec<(&str, &[u8])> = want.package.files.iter().map(|f| (f.path.as_str(), f.bytes.as_slice())).collect();
    assert!(got_files == want_files, "{what}: files differ from the hosted converter's ({} vs {} files)", got_files.len(), want_files.len());
    let got_losses: Vec<(&str, &str, &str)> = got.losses.iter().map(|l| (l.kind.as_str(), l.what.as_str(), l.why.as_str())).collect();
    let want_losses: Vec<(&str, &str, &str)> = want.losses.iter().map(|l| (l.kind.as_str(), l.what.as_str(), l.why.as_str())).collect();
    assert_eq!(got_losses, want_losses, "{what}: losses");
}

/// The textured fixture with its PNG re-encoded as WebP, as fal's TRELLIS.2
/// delivers textures: the hosted converter transcodes it back to PNG.
fn webp_textured() -> Scene {
    let mut scene = fixtures::textured();
    for img in &mut scene.images {
        let decoded = image::load_from_memory(&img.data).unwrap().to_rgba8();
        let mut webp = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
            .encode(decoded.as_raw(), decoded.width(), decoded.height(), image::ExtendedColorType::Rgba8)
            .unwrap();
        img.data = webp;
        img.format = ImageFormat::Webp;
    }
    scene
}

/// A KTX2 (Basis Universal) texture: mesh's own test file.
const DOT_C_KTX2: &[u8] = include_bytes!("../../../../litegen-core/src/mesh/testdata/ktx2/Dot_C.ktx2");

/// The textured fixture with its images replaced by a KTX2 texture, as a glTF
/// that requires `KHR_texture_basisu` carries them: the hosted converter
/// decodes it to PNG.
fn ktx2_textured() -> Scene {
    let mut scene = fixtures::textured();
    for img in &mut scene.images {
        img.data = DOT_C_KTX2.to_vec();
        img.format = ImageFormat::Ktx2;
    }
    scene
}

#[test]
fn default_convert_equals_the_hosted_pipeline() {
    let sources = [
        ("textured", fixtures::textured()),
        ("skinned", fixtures::skinned()),
        ("morph", fixtures::morph()),
        ("cube", fixtures::cube()),
        ("webp", webp_textured()),
        ("ktx2", ktx2_textured()),
    ];
    let srgb = litegen_mesh::FbxColorSpace::Srgb;
    for (name, scene) in sources {
        let glb = glb_of(&scene);
        let from = ReadOptions { from: Some(Format::Glb), ..Default::default() };
        for to in Format::ALL {
            let got = convert(Input::Bytes(&glb), &from, &WriteOptions::new(to)).unwrap();
            assert_same(&got, &hosted(&glb, MeshFormat::Glb, mesh(to), None, None, srgb, false), &format!("{name} to {to:?}"));
            let zipped = WriteOptions { package: Packaging::Zip, ..WriteOptions::new(to) };
            let got = convert(Input::Bytes(&glb), &from, &zipped).unwrap();
            assert_same(&got, &hosted(&glb, MeshFormat::Glb, mesh(to), None, None, srgb, true), &format!("{name} to {to:?} zipped"));
        }
    }
    let glb = glb_of(&fixtures::textured());
    for to in [Format::Stl, Format::Obj, Format::Ply] {
        let opts = WriteOptions { units: Some(Unit::Mm), up: Some(UpAxis::Z), ..WriteOptions::new(to) };
        let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &opts).unwrap();
        assert_same(&got, &hosted(&glb, MeshFormat::Glb, mesh(to), None, Some((0.001, true)), srgb, false), &format!("textured to {to:?} in mm, z up"));
        let stl = got.files[0].data.clone();
        let back = ReadOptions { from: Some(to), from_units: Some(Unit::Cm), from_up: Some(UpAxis::Z), ..Default::default() };
        let got = convert(Input::Bytes(&stl), &back, &WriteOptions::new(Format::Glb)).unwrap();
        assert_same(&got, &hosted(&stl, mesh(to), MeshFormat::Glb, Some((0.01, true)), None, srgb, false), &format!("{to:?} read in cm, z up"));
    }
}

#[test]
fn model_writes_equal_one_shot_convert() {
    for (name, scene) in [("webp", webp_textured()), ("ktx2", ktx2_textured())] {
        let glb = glb_of(&scene);
        let model = Model::read(Input::Bytes(&glb), &ReadOptions::default()).unwrap();
        // A framed write first: the cached scene must come out of it untouched.
        let framed = WriteOptions { units: Some(Unit::Mm), up: Some(UpAxis::Z), ..WriteOptions::new(Format::Stl) };
        let once = convert(Input::Bytes(&glb), &ReadOptions::default(), &framed).unwrap();
        assert_eq!(model.write(&framed).unwrap(), once, "{name} framed");
        for to in Format::ALL {
            for package in Packaging::ALL {
                let opts = WriteOptions { package, ..WriteOptions::new(to) };
                let once = convert(Input::Bytes(&glb), &ReadOptions::default(), &opts).unwrap();
                assert_eq!(model.write(&opts).unwrap(), once, "{name} to {to:?} as {package:?}");
            }
        }
    }
}

#[test]
fn package_modes() {
    let glb = glb_of(&fixtures::textured());
    let model = Model::read(Input::Bytes(&glb), &ReadOptions::default()).unwrap();
    for to in Format::ALL {
        let file = model.write(&WriteOptions::new(to)).unwrap();
        assert_eq!(file.files.len(), 1, "{to:?}");
        let only = &file.files[0];
        assert_eq!((only.name.as_str(), only.media_type, only.role), (format!("model.{}", to.as_str()).as_str(), to.media_type(), FileRole::Model));

        let files = model.write(&WriteOptions { package: Packaging::Files, ..WriteOptions::new(to) }).unwrap();
        assert_eq!(files.files[0].role, FileRole::Model);
        assert_eq!(files.files[0].media_type, to.media_type());
        assert_eq!(files.files.len() > 1, to.info().writes_sidecars, "{to:?}: sidecars as its info says");

        let zipped = model.write(&WriteOptions { package: Packaging::Zip, ..WriteOptions::new(to) }).unwrap();
        assert_eq!(zipped.files.len(), 1);
        let z = &zipped.files[0];
        assert_eq!((z.name.as_str(), z.media_type, z.role), (format!("model.{}.zip", to.as_str()).as_str(), "application/zip", FileRole::Model));
        assert!(z.data.starts_with(b"PK\x03\x04"));
        let entries: Vec<String> = zip::Archive::parse(&z.data).unwrap().entries.iter().map(|e| e.name.clone()).collect();
        let names: Vec<String> = files.files.iter().map(|f| f.name.clone()).collect();
        assert_eq!(entries, names, "{to:?}: the zip holds the package's files in order");
    }
    let obj = model.write(&WriteOptions { package: Packaging::Files, ..WriteOptions::new(Format::Obj) }).unwrap();
    let mtl = obj.files.iter().find(|f| f.name.ends_with(".mtl")).expect("an .mtl");
    assert_eq!((mtl.role, mtl.media_type), (FileRole::MaterialLibrary, "model/mtl"));
    let png = obj.files.iter().find(|f| f.name.ends_with(".png")).expect("a texture");
    assert_eq!((png.role, png.media_type), (FileRole::Texture, "image/png"));
    let gltf = model.write(&WriteOptions { package: Packaging::Files, ..WriteOptions::new(Format::Gltf) }).unwrap();
    let bin = gltf.files.iter().find(|f| f.name.ends_with(".bin")).expect("a .bin");
    assert_eq!((bin.role, bin.media_type), (FileRole::Buffer, "application/octet-stream"));
}

#[test]
fn write_option_rules() {
    let model = Model::read(Input::Bytes(&glb_of(&fixtures::skinned())), &ReadOptions::default()).unwrap();
    let cases = [
        (WriteOptions { units: Some(Unit::Mm), ..WriteOptions::new(Format::Glb) }, ErrorCode::ParamUnsupported, OptionName::Units,
            "`units` applies when `to` is stl, obj or ply, which carry no units or up axis; glb declares its own"),
        (WriteOptions { up: Some(UpAxis::Z), ..WriteOptions::new(Format::Fbx) }, ErrorCode::ParamUnsupported, OptionName::Up,
            "`up` applies when `to` is stl, obj or ply, which carry no units or up axis; fbx declares its own"),
        (WriteOptions { fps: Some(24), ..WriteOptions::new(Format::Glb) }, ErrorCode::ParamUnsupported, OptionName::Fps,
            "`fps` applies when `to` is fbx, dae, usdz, usda or usdc, which key animation at a fixed rate"),
        (WriteOptions { fps: Some(0), ..WriteOptions::new(Format::Fbx) }, ErrorCode::ParamInvalid, OptionName::Fps,
            "`fps` is 0; it takes a whole number of frames per second, 1 or more"),
        (WriteOptions { fbx_color_space: Some(FbxColorSpace::Linear), ..WriteOptions::new(Format::Obj) }, ErrorCode::ParamUnsupported, OptionName::FbxColorSpace,
            "`fbx_color_space` applies when `to` is fbx"),
        (WriteOptions { fbx_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Usda) }, ErrorCode::ParamUnsupported, OptionName::FbxLightUnits,
            "`fbx_light_units` applies when `to` is fbx"),
        (WriteOptions { usd_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Fbx) }, ErrorCode::ParamUnsupported, OptionName::UsdLightUnits,
            "`usd_light_units` applies when `to` is usdz, usda or usdc"),
        (WriteOptions { dae_morphs: Some(DaeMorphs::Keep), ..WriteOptions::new(Format::Glb) }, ErrorCode::ParamUnsupported, OptionName::DaeMorphs,
            "`dae_morphs` applies when `to` is dae"),
        (WriteOptions { dae_morphs: Some(DaeMorphs::Bake), ..WriteOptions::new(Format::Fbx) }, ErrorCode::ParamUnsupported, OptionName::DaeMorphs,
            "`dae_morphs` applies when `to` is dae"),
        (WriteOptions { usdz_root: Some(UsdzRoot::Usdc), ..WriteOptions::new(Format::Usda) }, ErrorCode::ParamUnsupported, OptionName::UsdzRoot,
            "`usdz_root` applies when `to` is usdz"),
        (WriteOptions { usdz_root: Some(UsdzRoot::Usda), ..WriteOptions::new(Format::Glb) }, ErrorCode::ParamUnsupported, OptionName::UsdzRoot,
            "`usdz_root` applies when `to` is usdz"),
        (WriteOptions { usd_clips: Some(UsdClips::All), ..WriteOptions::new(Format::Glb) }, ErrorCode::ParamUnsupported, OptionName::UsdClips,
            "`usd_clips` applies when `to` is usda, usdc or usdz"),
        (WriteOptions { usd_clips: Some(UsdClips::First), ..WriteOptions::new(Format::Fbx) }, ErrorCode::ParamUnsupported, OptionName::UsdClips,
            "`usd_clips` applies when `to` is usda, usdc or usdz"),
    ];
    for (opts, code, param, message) in cases {
        let e = model.write(&opts).unwrap_err();
        assert_eq!((e.code, e.param), (code, Some(param)), "{opts:?}");
        assert_eq!(e.message(), message);
    }
}

/// `dae_morphs` is the write's alone: a Collada source does not make it
/// apply, unlike the light units, which apply at either end.
#[test]
fn dae_morphs_applies_to_the_written_dae_only() {
    let dae = convert(Input::Bytes(&glb_of(&fixtures::morph())), &ReadOptions::default(), &WriteOptions::new(Format::Dae)).unwrap();
    let dae = &dae.files[0].data;
    let keep = |to| WriteOptions { dae_morphs: Some(DaeMorphs::Keep), ..WriteOptions::new(to) };
    let e = convert(Input::Bytes(dae), &ReadOptions::default(), &keep(Format::Glb)).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(OptionName::DaeMorphs)));
    assert_eq!(e.message(), "`dae_morphs` applies when `to` is dae");
    convert(Input::Bytes(dae), &ReadOptions::default(), &keep(Format::Dae)).unwrap();
}

/// mesh's write of `scene` as one Collada file with this morph choice.
fn mesh_dae(scene: &Scene, dae_morphs: litegen_mesh::DaeMorphs) -> litegen_mesh::SceneWrite {
    write_scene(scene, MeshFormat::Dae, litegen_mesh::WriteOptions { dae_morphs, ..Default::default() }).unwrap()
}

fn morph_losses(c: &Conversion) -> usize {
    c.losses.iter().filter(|l| l.kind == LossKind::MorphTargets).count()
}

/// `dae_morphs` reaches mesh as given: the package writes the file and the
/// losses mesh does with the same choice, `bake` is the default, and `keep`
/// changes a morphed model's file and drops its morph losses. No file is
/// pinned by hash: Collada's text differs between platforms' maths
/// libraries, so each side is mesh's on the same machine.
#[test]
fn dae_morphs_reach_mesh() {
    use litegen_mesh::LightUnits::Photometric;
    let glb = glb_of(&fixtures::morph());
    let morph = mesh_read(&glb, MeshFormat::Glb, Photometric, Photometric);
    let model = Model::read(Input::Bytes(&glb), &ReadOptions::default()).unwrap();
    let write = |dae_morphs| model.write(&WriteOptions { dae_morphs, ..WriteOptions::new(Format::Dae) }).unwrap();
    let (bake, keep) = (write(Some(DaeMorphs::Bake)), write(Some(DaeMorphs::Keep)));
    assert_same(&bake, &mesh_dae(&morph, litegen_mesh::DaeMorphs::Bake), "bake");
    assert_same(&keep, &mesh_dae(&morph, litegen_mesh::DaeMorphs::Keep), "keep");
    assert_eq!(write(None), bake, "bake is the default");
    assert_ne!(keep.files[0].data, bake.files[0].data, "keep changes a morphed model's file");
    assert!(morph_losses(&bake) > 0, "bake reports the morphs it bakes: {:?}", bake.losses);
    assert_eq!(morph_losses(&keep), 0, "keep reports none: {:?}", keep.losses);
    let one_shot = WriteOptions { dae_morphs: Some(DaeMorphs::Keep), ..WriteOptions::new(Format::Dae) };
    assert_eq!(convert(Input::Bytes(&glb), &ReadOptions::default(), &one_shot).unwrap(), keep, "convert takes it as Model::write does");
    // A model with no morphs writes the same file either way.
    let cube = Model::read(Input::Bytes(&glb_of(&fixtures::cube())), &ReadOptions::default()).unwrap();
    let cube_dae = |dae_morphs| cube.write(&WriteOptions { dae_morphs, ..WriteOptions::new(Format::Dae) }).unwrap();
    assert_eq!(cube_dae(Some(DaeMorphs::Keep)), cube_dae(None), "keep changes nothing without morphs");
}

/// The drawn geometry of `scene` at rest and at each of `times` in its first
/// clip: every primitive's world positions and normals, in drawing order.
#[allow(clippy::type_complexity)]
fn posed(scene: &Scene, times: &[f32]) -> Vec<Vec<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>)>> {
    use litegen_mesh::pose;
    let mut poses = vec![pose::default_pose(scene)];
    poses.extend(times.iter().map(|t| pose::sample_pose(scene, 0, *t)));
    poses
        .iter()
        .map(|p| pose::world_primitives(scene, p).unwrap().into_iter().map(|w| (w.positions, w.normals)).collect())
        .collect()
}

/// Whether two drawings agree to within `eps`, primitive for primitive.
#[allow(clippy::type_complexity)]
fn alike(a: &[Vec<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>)>], b: &[Vec<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>)>], eps: f32) -> bool {
    let close = |x: &[[f32; 3]], y: &[[f32; 3]]| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| (0..3).all(|i| (p[i] - q[i]).abs() <= eps));
    a.len() == b.len()
        && a.iter().zip(b).all(|(pa, pb)| {
            pa.len() == pb.len()
                && pa.iter().zip(pb).all(|((ap, an), (bp, bn))| {
                    close(ap, bp)
                        && match (an, bn) {
                            (Some(an), Some(bn)) => close(an, bn),
                            (None, None) => true,
                            _ => false,
                        }
                })
        })
}

/// A Collada file written with `keep` reads back through the package with
/// its morph targets: their names, their default weights, and the same
/// drawing at rest and through the weight animation. With `bake` the targets
/// are gone and the animation no longer moves the drawing. A morphed mesh of
/// several primitives reads back as one mesh each, so meshes are not
/// counted.
#[test]
fn kept_morphs_round_trip_through_the_package() {
    use litegen_mesh::LightUnits::Photometric;
    let fixture = fixtures::morph();
    let glb = glb_of(&fixture);
    let times = fixtures::sample_times(&fixture);
    let want = posed(&fixture, &times);
    let back = |dae_morphs| {
        let dae = convert(Input::Bytes(&glb), &ReadOptions::default(), &WriteOptions { dae_morphs, ..WriteOptions::new(Format::Dae) }).unwrap();
        let glb = convert(Input::Bytes(&dae.files[0].data), &ReadOptions::default(), &WriteOptions::new(Format::Glb)).unwrap();
        mesh_read(&glb.files[0].data, MeshFormat::Glb, Photometric, Photometric)
    };
    let kept = back(Some(DaeMorphs::Keep));
    let morphed: Vec<_> = kept.meshes.iter().filter(|m| m.target_count() > 0).collect();
    assert!(!morphed.is_empty(), "keep's targets read back");
    for m in morphed {
        assert_eq!(m.target_names, fixture.meshes[0].target_names, "target names");
        assert_eq!(m.weights, fixture.meshes[0].weights, "default weights");
    }
    assert!(alike(&posed(&kept, &times), &want, 1e-5), "keep draws the fixture at rest and at every sample time");

    let baked = back(Some(DaeMorphs::Bake));
    assert!(baked.meshes.iter().all(|m| m.target_count() == 0), "bake writes no targets");
    assert!(!alike(&posed(&baked, &times), &want, 1e-5), "bake's drawing does not follow the weight animation");
}

#[test]
fn convert_takes_one_fbx_colour_space_for_both_ends() {
    let fbx = write_scene(&fixtures::textured(), MeshFormat::Fbx, litegen_mesh::WriteOptions::default()).unwrap().package.files.remove(0).bytes;
    let linear = |to| WriteOptions { fbx_color_space: Some(FbxColorSpace::Linear), ..WriteOptions::new(to) };
    // Given on the write side only, it still reads the fbx source as linear.
    let got = convert(Input::Bytes(&fbx), &ReadOptions::default(), &linear(Format::Glb)).unwrap();
    assert_same(&got, &hosted(&fbx, MeshFormat::Fbx, MeshFormat::Glb, None, None, litegen_mesh::FbxColorSpace::Linear, false), "fbx read as linear");
    let got = convert(Input::Bytes(&fbx), &ReadOptions::default(), &linear(Format::Fbx)).unwrap();
    assert_same(&got, &hosted(&fbx, MeshFormat::Fbx, MeshFormat::Fbx, None, None, litegen_mesh::FbxColorSpace::Linear, false), "fbx to fbx, linear at both ends");
    let glb = glb_of(&fixtures::textured());
    let e = convert(Input::Bytes(&glb), &ReadOptions::default(), &linear(Format::Obj)).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(OptionName::FbxColorSpace)));
    assert_eq!(e.message(), "`fbx_color_space` applies when `from` or `to` is fbx");
    let into_fbx = convert(Input::Bytes(&glb), &ReadOptions { fbx_color_space: Some(FbxColorSpace::Linear), ..Default::default() }, &WriteOptions::new(Format::Fbx)).unwrap();
    assert_same(&into_fbx, &hosted(&glb, MeshFormat::Glb, MeshFormat::Fbx, None, None, litegen_mesh::FbxColorSpace::Linear, false), "glb to fbx, linear");
}

/// mesh's read of `bytes` with these light units, as one file.
fn mesh_read(bytes: &[u8], from: MeshFormat, fbx: litegen_mesh::LightUnits, usd: litegen_mesh::LightUnits) -> Scene {
    let options = litegen_mesh::ReadOptions { fbx_color_space: litegen_mesh::FbxColorSpace::Srgb, fbx_light_units: fbx, usd_light_units: usd };
    litegen_mesh::read_scene_with(bytes, from, &litegen_mesh::NoSidecars, options).unwrap().scene
}

/// mesh's write of `scene` with these light units, as one file.
fn mesh_write(scene: &Scene, to: MeshFormat, fbx: litegen_mesh::LightUnits, usd: litegen_mesh::LightUnits) -> Vec<u8> {
    let options = litegen_mesh::WriteOptions {
        self_contained: true,
        fps: None,
        fbx_color_space: litegen_mesh::FbxColorSpace::Srgb,
        fbx_light_units: fbx,
        usd_light_units: usd,
        dae_morphs: litegen_mesh::DaeMorphs::Bake,
        usdz_root: litegen_mesh::UsdzRoot::Usda,
        usd_clips: None,
    };
    write_scene(scene, to, options).unwrap().package.files.remove(0).bytes
}

/// `fbx_light_units` and `usd_light_units` reach mesh as given, at either
/// end: the package writes and reads what mesh does with the same choice,
/// and Blender's units change a lit FBX.
#[test]
fn light_units_reach_mesh_at_either_end() {
    use litegen_mesh::LightUnits::{Blender, Photometric};
    let glb = glb_of(&fixtures::staged());
    let staged = mesh_read(&glb, MeshFormat::Glb, Photometric, Photometric);
    let model = Model::read(Input::Bytes(&glb), &ReadOptions::default()).unwrap();
    let write = |opts: WriteOptions| model.write(&opts).unwrap().files.remove(0).data;

    let fbx_blender = write(WriteOptions { fbx_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Fbx) });
    assert_eq!(fbx_blender, mesh_write(&staged, MeshFormat::Fbx, Blender, Photometric), "fbx written in blender's units");
    assert_ne!(fbx_blender, write(WriteOptions::new(Format::Fbx)), "blender's units change a lit fbx");
    let photometric = write(WriteOptions { fbx_light_units: Some(LightUnits::Photometric), ..WriteOptions::new(Format::Fbx) });
    assert_eq!(photometric, write(WriteOptions::new(Format::Fbx)), "photometric is the default");
    for (to, mesh_to) in [(Format::Usda, MeshFormat::Usda), (Format::Usdz, MeshFormat::Usdz)] {
        let usd_blender = write(WriteOptions { usd_light_units: Some(LightUnits::Blender), ..WriteOptions::new(to) });
        assert_eq!(usd_blender, mesh_write(&staged, mesh_to, Photometric, Blender), "{to:?} written in blender's units");
        assert_ne!(usd_blender, write(WriteOptions::new(to)), "blender's units change a lit {to:?}");
    }
    // Reading litegen's own files back, its `customData` wins under either
    // unit, so the read checks above are of the plumbing only.

    let as_glb = |bytes: &[u8], opts: ReadOptions| {
        Model::read(Input::Bytes(bytes), &opts).unwrap().write(&WriteOptions::new(Format::Glb)).unwrap().files.remove(0).data
    };
    let fbx = mesh_write(&staged, MeshFormat::Fbx, Photometric, Photometric);
    let got = as_glb(&fbx, ReadOptions { fbx_light_units: Some(LightUnits::Blender), ..Default::default() });
    let want = mesh_write(&mesh_read(&fbx, MeshFormat::Fbx, Blender, Photometric), MeshFormat::Glb, Photometric, Photometric);
    assert_eq!(got, want, "fbx read in blender's units");
    let usda = mesh_write(&staged, MeshFormat::Usda, Photometric, Photometric);
    let got = as_glb(&usda, ReadOptions { usd_light_units: Some(LightUnits::Blender), ..Default::default() });
    let want = mesh_write(&mesh_read(&usda, MeshFormat::Usda, Photometric, Blender), MeshFormat::Glb, Photometric, Photometric);
    assert_eq!(got, want, "usda read in blender's units");
}

/// The hosted converter's light units, end for end: the package's one-call
/// convert answers as `convert_bytes` does for the same choice.
#[test]
fn light_units_equal_the_hosted_pipeline() {
    use litegen_mesh::LightUnits::{Blender, Photometric};
    let glb = glb_of(&fixtures::staged());
    let staged = mesh_read(&glb, MeshFormat::Glb, Photometric, Photometric);
    let fbx = mesh_write(&staged, MeshFormat::Fbx, Photometric, Photometric);
    let usda = mesh_write(&staged, MeshFormat::Usda, Photometric, Photometric);
    let usdc = mesh_write(&staged, MeshFormat::Usdc, Photometric, Photometric);
    let read_fbx = ReadOptions { fbx_light_units: Some(LightUnits::Blender), ..Default::default() };
    let read_usd = ReadOptions { usd_light_units: Some(LightUnits::Blender), ..Default::default() };
    let write_fbx = |to| WriteOptions { fbx_light_units: Some(LightUnits::Blender), ..WriteOptions::new(to) };
    let write_usd = |to| WriteOptions { usd_light_units: Some(LightUnits::Blender), ..WriteOptions::new(to) };
    let cases = [
        ("glb to fbx", &glb, Format::Glb, ReadOptions::default(), write_fbx(Format::Fbx), (Blender, Photometric)),
        ("fbx to glb", &fbx, Format::Fbx, read_fbx.clone(), WriteOptions::new(Format::Glb), (Blender, Photometric)),
        ("fbx to fbx", &fbx, Format::Fbx, read_fbx, WriteOptions::new(Format::Fbx), (Blender, Photometric)),
        ("glb to usda", &glb, Format::Glb, ReadOptions::default(), write_usd(Format::Usda), (Photometric, Blender)),
        ("glb to usdz", &glb, Format::Glb, ReadOptions::default(), write_usd(Format::Usdz), (Photometric, Blender)),
        ("glb to usdc", &glb, Format::Glb, ReadOptions::default(), write_usd(Format::Usdc), (Photometric, Blender)),
        ("usdc to fbx, both", &usdc, Format::Usdc, read_usd.clone(), write_fbx(Format::Fbx), (Blender, Blender)),
        ("usda to fbx, both", &usda, Format::Usda, read_usd, write_fbx(Format::Fbx), (Blender, Blender)),
    ];
    let srgb = litegen_mesh::FbxColorSpace::Srgb;
    for (what, bytes, from, read, write, units) in cases {
        let got = convert(Input::Bytes(bytes), &read, &write).unwrap();
        let defaults = litegen_mesh::WriteOptions::default();
        assert_same(&got, &hosted_lit(bytes, mesh(from), mesh(write.to), None, None, srgb, false, units, defaults), what);
    }
}

/// The hosted converter's `dae_morphs`: the package's one-call convert
/// answers as `convert_bytes` does for each choice, as one file and zipped,
/// for a model with morph targets and one without.
#[test]
fn dae_morphs_equal_the_hosted_pipeline() {
    use litegen_mesh::LightUnits::Photometric;
    let srgb = litegen_mesh::FbxColorSpace::Srgb;
    let choices = [
        (None, litegen_mesh::DaeMorphs::Bake),
        (Some(DaeMorphs::Bake), litegen_mesh::DaeMorphs::Bake),
        (Some(DaeMorphs::Keep), litegen_mesh::DaeMorphs::Keep),
    ];
    for (name, scene) in [("morph", fixtures::morph()), ("cube", fixtures::cube())] {
        let glb = glb_of(&scene);
        for (ours, theirs) in choices {
            for (package, zipped) in [(Packaging::File, false), (Packaging::Zip, true)] {
                let write = WriteOptions { dae_morphs: ours, package, ..WriteOptions::new(Format::Dae) };
                let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &write).unwrap();
                let choices = litegen_mesh::WriteOptions { dae_morphs: theirs, ..Default::default() };
                let want = hosted_lit(&glb, MeshFormat::Glb, MeshFormat::Dae, None, None, srgb, zipped, (Photometric, Photometric), choices);
                assert_same(&got, &want, &format!("{name}, dae_morphs {ours:?}, {package:?}"));
            }
        }
    }
}

/// The hosted converter's `usdz_root` and `usd_clips`: the package's one-call
/// convert answers as `convert_bytes` does for each choice, as one file and
/// zipped, for every USD file it writes.
#[test]
fn usd_choices_equal_the_hosted_pipeline() {
    use litegen_mesh::LightUnits::Photometric;
    let srgb = litegen_mesh::FbxColorSpace::Srgb;
    let roots = [
        (None, litegen_mesh::UsdzRoot::Usda),
        (Some(UsdzRoot::Usda), litegen_mesh::UsdzRoot::Usda),
        (Some(UsdzRoot::Usdc), litegen_mesh::UsdzRoot::Usdc),
    ];
    let clips = [
        (None, None),
        (Some(UsdClips::All), Some(litegen_mesh::UsdClips::All)),
        (Some(UsdClips::First), Some(litegen_mesh::UsdClips::First)),
    ];
    for (name, scene) in [("skinned", fixtures::skinned()), ("textured", fixtures::textured()), ("clips", fixtures::clips())] {
        let glb = glb_of(&scene);
        for (package, zipped) in [(Packaging::File, false), (Packaging::Zip, true)] {
            let hosted_as = |to: MeshFormat, choices: litegen_mesh::WriteOptions| {
                hosted_lit(&glb, MeshFormat::Glb, to, None, None, srgb, zipped, (Photometric, Photometric), choices)
            };
            for (ours, theirs) in roots {
                let write = WriteOptions { usdz_root: ours, package, ..WriteOptions::new(Format::Usdz) };
                let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &write).unwrap();
                let want = hosted_as(MeshFormat::Usdz, litegen_mesh::WriteOptions { usdz_root: theirs, ..Default::default() });
                assert_same(&got, &want, &format!("{name}, usdz_root {ours:?}, {package:?}"));
            }
            for to in [Format::Usda, Format::Usdc, Format::Usdz] {
                for (ours, theirs) in clips {
                    let write = WriteOptions { usd_clips: ours, package, ..WriteOptions::new(to) };
                    let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &write).unwrap();
                    let want = hosted_as(mesh(to), litegen_mesh::WriteOptions { usd_clips: theirs, ..Default::default() });
                    assert_same(&got, &want, &format!("{name}, {to:?}, usd_clips {ours:?}, {package:?}"));
                }
            }
        }
    }
}

/// `usdz_root` reaches the package: its first entry, the root layer, is the
/// crate with `usdc`, and the text with `usda` or by default.
#[test]
fn usdz_root_reaches_the_package() {
    let glb = glb_of(&fixtures::textured());
    let root_of = |usdz_root| {
        let write = WriteOptions { usdz_root, ..WriteOptions::new(Format::Usdz) };
        let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &write).unwrap();
        zip::Archive::parse(&got.files[0].data).unwrap().entries[0].name.clone()
    };
    assert_eq!(root_of(None), "model.usda");
    assert_eq!(root_of(Some(UsdzRoot::Usda)), "model.usda");
    assert_eq!(root_of(Some(UsdzRoot::Usdc)), "model.usdc");
}

/// `usd_clips` reaches the write: a scene of several clips keeps them all,
/// as the variant set `Animation`, with `all`, and the first alone with
/// `first`. Absent, a usda or usdc keeps them all and a usdz the first, as
/// mesh's `WriteOptions::clips_for` decides.
#[test]
fn usd_clips_reaches_the_write() {
    let glb = glb_of(&fixtures::clips());
    let write = |to, usd_clips| {
        let options = WriteOptions { usd_clips, ..WriteOptions::new(to) };
        convert(Input::Bytes(&glb), &ReadOptions::default(), &options).unwrap().files.remove(0).data
    };
    for to in [Format::Usda, Format::Usdc, Format::Usdz] {
        let (all, first) = (write(to, Some(UsdClips::All)), write(to, Some(UsdClips::First)));
        assert_ne!(all, first, "{to:?}: all and first write the same file");
        let default = if to == Format::Usdz { &first } else { &all };
        assert!(write(to, None) == *default, "{to:?}: the default is not the container's");
    }
    let text = String::from_utf8(write(Format::Usda, Some(UsdClips::All))).unwrap();
    assert!(text.contains("variantSet \"Animation\""), "no Animation variant set in:\n{text:.600}");
}

/// usdc writes a crate, served as litegen serves one, and packages its
/// textures beside it as usda does.
#[test]
fn usdc_writes_a_crate() {
    let glb = glb_of(&fixtures::textured());
    let one = convert(Input::Bytes(&glb), &ReadOptions::default(), &WriteOptions::new(Format::Usdc)).unwrap();
    assert_eq!(one.format, Format::Usdc);
    let model = &one.files[0];
    assert_eq!((model.name.as_str(), model.media_type, model.role), ("model.usdc", "application/octet-stream", FileRole::Model));
    assert!(model.data.starts_with(b"PXR-USDC"));
    let files = WriteOptions { package: Packaging::Files, ..WriteOptions::new(Format::Usdc) };
    let files = convert(Input::Bytes(&glb), &ReadOptions::default(), &files).unwrap();
    assert_eq!(files.files[0].name, "model.usdc");
    assert!(files.files.iter().any(|f| f.role == FileRole::Texture), "{:?}", files.files.iter().map(|f| &f.name).collect::<Vec<_>>());
}

/// `usdz_root` and `usd_clips` are the write's alone, as `dae_morphs` is: a
/// USD source does not make them apply.
#[test]
fn usd_choices_apply_to_the_written_file_only() {
    let glb = glb_of(&fixtures::skinned());
    let usdz = convert(Input::Bytes(&glb), &ReadOptions::default(), &WriteOptions::new(Format::Usdz)).unwrap().files.remove(0).data;
    let cases = [
        (WriteOptions { usdz_root: Some(UsdzRoot::Usdc), ..WriteOptions::new(Format::Usda) }, OptionName::UsdzRoot,
            "`usdz_root` applies when `to` is usdz"),
        (WriteOptions { usd_clips: Some(UsdClips::All), ..WriteOptions::new(Format::Glb) }, OptionName::UsdClips,
            "`usd_clips` applies when `to` is usda, usdc or usdz"),
    ];
    for (write, param, message) in cases {
        let e = convert(Input::Bytes(&usdz), &ReadOptions::default(), &write).unwrap_err();
        assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(param)), "{write:?}");
        assert_eq!(e.message(), message);
    }
}

/// As `fbx_color_space`: one choice for whichever end is that format, given
/// on either side.
#[test]
fn convert_takes_one_light_units_choice_for_both_ends() {
    let glb = glb_of(&fixtures::staged());
    let staged = Model::read(Input::Bytes(&glb), &ReadOptions::default()).unwrap();
    let fbx = staged.write(&WriteOptions::new(Format::Fbx)).unwrap().files.remove(0).data;
    let blender_fbx = WriteOptions { fbx_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Glb) };
    // Given on the write side only, it still reads the fbx source in blender's units.
    let got = convert(Input::Bytes(&fbx), &ReadOptions::default(), &blender_fbx).unwrap();
    let read_so = ReadOptions { fbx_light_units: Some(LightUnits::Blender), ..Default::default() };
    let want = Model::read(Input::Bytes(&fbx), &read_so).unwrap().write(&WriteOptions::new(Format::Glb)).unwrap();
    assert_eq!(got, want);
    // Given on the read side of a glb source, it reaches the fbx it writes.
    let got = convert(Input::Bytes(&glb), &read_so, &WriteOptions::new(Format::Fbx)).unwrap();
    assert_eq!(got, staged.write(&WriteOptions { fbx_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Fbx) }).unwrap());
    // A usd source and an fbx target take one choice each.
    let usda = staged.write(&WriteOptions::new(Format::Usda)).unwrap().files.remove(0).data;
    let usd_blender = ReadOptions { usd_light_units: Some(LightUnits::Blender), ..Default::default() };
    convert(Input::Bytes(&usda), &usd_blender, &WriteOptions { fbx_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Fbx) }).unwrap();
    for (opts, param, message) in [
        (WriteOptions { usd_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Obj) }, OptionName::UsdLightUnits,
            "`usd_light_units` applies when `from` or `to` is usdz, usda or usdc"),
        (WriteOptions { fbx_light_units: Some(LightUnits::Blender), ..WriteOptions::new(Format::Obj) }, OptionName::FbxLightUnits,
            "`fbx_light_units` applies when `from` or `to` is fbx"),
    ] {
        let e = convert(Input::Bytes(&glb), &ReadOptions::default(), &opts).unwrap_err();
        assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(param)));
        assert_eq!(e.message(), message);
    }
}

fn extent(data: &[u8], f: Format) -> [f64; 3] {
    let m = Model::read(Input::Bytes(data), &ReadOptions { from: Some(f), ..Default::default() }).unwrap();
    let b = m.info().unwrap().bounds.unwrap();
    [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]]
}

#[test]
fn units_and_up_reach_the_file() {
    let box_stl = b"solid box\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 2 0 0\n   vertex 0 1 4\n  endloop\n endfacet\nendsolid box\n";
    let model = Model::read(Input::Bytes(box_stl), &ReadOptions::default()).unwrap();
    let mm = model.write(&WriteOptions { units: Some(Unit::Mm), ..WriteOptions::new(Format::Stl) }).unwrap();
    let e = extent(&mm.files[0].data, Format::Stl);
    for (got, want) in e.iter().zip([2000.0, 1000.0, 4000.0]) {
        assert!((got - want).abs() < 1e-3, "{e:?}");
    }
    // +Y up becomes +Z up and the front (+Z) becomes -Y: (x, y, z) -> (x, -z, y).
    let z_up = model.write(&WriteOptions { up: Some(UpAxis::Z), ..WriteOptions::new(Format::Obj) }).unwrap();
    let e = extent(&z_up.files[0].data, Format::Obj);
    for (got, want) in e.iter().zip([2.0, 4.0, 1.0]) {
        assert!((got - want).abs() < 1e-6, "{e:?}");
    }
}

#[test]
fn fbx_colour_space_and_fps_reach_the_writer() {
    let textured = Model::read(Input::Bytes(&glb_of(&fixtures::textured())), &ReadOptions::default()).unwrap();
    let as_fbx = |c: Option<FbxColorSpace>| textured.write(&WriteOptions { fbx_color_space: c, ..WriteOptions::new(Format::Fbx) }).unwrap().files.remove(0).data;
    assert_ne!(as_fbx(Some(FbxColorSpace::Srgb)), as_fbx(Some(FbxColorSpace::Linear)));
    assert_eq!(as_fbx(None), as_fbx(Some(FbxColorSpace::Srgb)));
    let skinned = Model::read(Input::Bytes(&glb_of(&fixtures::skinned())), &ReadOptions::default()).unwrap();
    for to in [Format::Fbx, Format::Dae, Format::Usda] {
        let at = |fps: Option<u32>| skinned.write(&WriteOptions { fps, ..WriteOptions::new(to) }).unwrap().files.remove(0).data;
        assert_ne!(at(Some(24)), at(None), "{to:?} at 24 fps");
        assert_eq!(at(Some(30)), at(None), "{to:?}: 30 fps is the default");
    }
}

#[test]
fn undecodable_texture_is_an_image_formats_loss() {
    let mut scene = fixtures::textured();
    scene.images[0].data = b"RIFF\x1a\x00\x00\x00WEBPVP8 \x0e\x00\x00\x00not a real webp".to_vec();
    scene.images[0].format = ImageFormat::Webp;
    let glb = glb_of(&scene);
    let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &WriteOptions::new(Format::Obj)).unwrap();
    let loss = got.losses.iter().find(|l| l.kind == LossKind::ImageFormats).expect("an image_formats loss");
    assert!(loss.what.starts_with("texture "), "{loss:?}");
    assert_eq!(loss.why, "litegen could not decode it to PNG for this container");
}

#[test]
fn an_undecodable_ktx2_is_named_with_the_decoders_reason() {
    let mut scene = fixtures::textured();
    // KTX2's file identifier, then nothing a KTX2 decoder can read.
    scene.images[0].data = [&b"\xABKTX 20\xBB\r\n\x1A\n"[..], b"not a real ktx2"].concat();
    scene.images[0].format = ImageFormat::Ktx2;
    let glb = glb_of(&scene);
    let srgb = litegen_mesh::FbxColorSpace::Srgb;
    for to in [Format::Obj, Format::Glb] {
        let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &WriteOptions::new(to)).unwrap();
        assert_same(&got, &hosted(&glb, MeshFormat::Glb, mesh(to), None, None, srgb, false), &format!("a broken KTX2 to {to:?}"));
        let loss = got.losses.iter().find(|l| l.kind == LossKind::ImageFormats).expect("an image_formats loss");
        assert!(loss.why.starts_with("litegen could not decode it to PNG for this container: "), "{loss:?}");
    }
}

/// A KTX2 that transcoding would take past the texture budget is left out
/// and named as the hosted converter names it. The scene is at the budget
/// already, as in mesh's own test of the budget: a PNG's signature, then
/// zeros. The reader takes it for the PNG it says it is (an image already PNG
/// is never decoded), so its bytes count in full.
#[test]
fn a_ktx2_past_the_texture_budget_is_named_as_the_hosted_converter_names_it() {
    use litegen_mesh::scene::{Image, Texture, TextureRef};
    let mut scene = fixtures::textured();
    let mut stored = vec![0; normalize::TEXTURE_BUDGET];
    stored[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    scene.images.push(Image { name: Some("stored".into()), format: ImageFormat::Png, data: stored });
    scene.images.push(Image { name: Some("basis".into()), format: ImageFormat::Ktx2, data: DOT_C_KTX2.to_vec() });
    scene.textures.push(Texture { name: None, image: 1, sampler: Default::default() });
    scene.textures.push(Texture { name: None, image: 2, sampler: Default::default() });
    scene.materials[1].emissive_texture = Some(TextureRef { texture: 1, uv_set: 0, transform: None });
    scene.materials[1].base_color_texture = Some(TextureRef { texture: 2, uv_set: 0, transform: None });
    let glb = glb_of(&scene);
    let got = convert(Input::Bytes(&glb), &ReadOptions::default(), &WriteOptions::new(Format::Glb)).unwrap();
    let srgb = litegen_mesh::FbxColorSpace::Srgb;
    assert_same(&got, &hosted(&glb, MeshFormat::Glb, MeshFormat::Glb, None, None, srgb, false), "a KTX2 past the budget");
    assert!(got.losses.iter().any(|l| l.kind == LossKind::ImageFormats && l.why == normalize::OVER_BUDGET), "{:?}", got.losses);
}

#[test]
fn losses_name_what_a_poorer_target_drops() {
    let model = Model::read(Input::Bytes(&glb_of(&fixtures::skinned())), &ReadOptions::default()).unwrap();
    let stl = model.write(&WriteOptions::new(Format::Stl)).unwrap();
    let kinds: Vec<LossKind> = stl.losses.iter().map(|l| l.kind).collect();
    assert!(kinds.contains(&LossKind::Animation) && kinds.contains(&LossKind::Skins), "{kinds:?}");
    assert!(model.write(&WriteOptions::new(Format::Glb)).unwrap().losses.is_empty(), "glb keeps everything");
}

#[test]
fn conversion_warnings_are_the_readers() {
    let obj = b"mtllib missing.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let model = Model::read(Input::Bytes(obj), &ReadOptions::default()).unwrap();
    let got = model.write(&WriteOptions::new(Format::Glb)).unwrap();
    assert_eq!(got.warnings, model.warnings());
    assert!(!got.warnings.is_empty());
}
