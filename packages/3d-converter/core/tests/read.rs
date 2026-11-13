//! Reading: one file or a set of files (a model and its sidecars, or a zip of
//! them), which file is the model, each file's role, the read options' rules
//! and frame, and the model's facts.

use std::collections::BTreeSet;

use litegen_3d_converter::{
    check_input_size, convert, ErrorCode, FbxColorSpace, FileRole, Format, HumanoidNaming, Input, InputFile, LightUnits, Model, OptionName,
    ReadOptions, TextureMap, Unit, UpAxis, VrmBone, WriteOptions as ConvertWriteOptions, MAX_INPUT_BYTES,
};
use litegen_mesh::{fixtures, inspect, package::Package, write_scene, zip, MeshFormat, Scene, WriteOptions};

fn written(scene: &Scene, f: Format, self_contained: bool) -> Package {
    let opts = WriteOptions {
        self_contained,
        fps: None,
        fbx_color_space: litegen_mesh::FbxColorSpace::Srgb,
        fbx_light_units: litegen_mesh::LightUnits::Photometric,
        usd_light_units: litegen_mesh::LightUnits::Photometric,
        dae_morphs: litegen_mesh::DaeMorphs::Bake,
        usdz_root: litegen_mesh::UsdzRoot::Usda,
        usd_clips: None,
    };
    write_scene(scene, MeshFormat::parse(f.as_str()).unwrap(), opts).unwrap().package
}

fn one(scene: &Scene, f: Format) -> Vec<u8> {
    written(scene, f, true).files.remove(0).bytes
}

fn files_of(package: Package) -> Vec<InputFile<'static>> {
    package.files.into_iter().map(|f| InputFile { name: f.path, data: f.bytes.into() }).collect()
}

fn role_of(model: &Model, name: &str) -> FileRole {
    model.files().iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name} not listed: {:?}", model.files())).role
}

fn read(input: Input<'_>) -> Result<Model, litegen_3d_converter::Error> {
    Model::read(input, &ReadOptions::default())
}

#[test]
fn a_model_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Model>();
}

#[test]
fn reads_one_file_by_detection() {
    let scene = fixtures::skinned();
    let glb = one(&scene, Format::Glb);
    let m = read(Input::Bytes(&glb)).unwrap();
    assert_eq!(m.format(), Format::Glb);
    assert_eq!(m.info().unwrap().triangles as usize, scene.triangle_count());
    assert_eq!(m.files().len(), 1);
    assert_eq!((m.files()[0].name.as_str(), m.files()[0].role, m.files()[0].size), ("model.glb", FileRole::Model, glb.len() as u64));

    let named = Model::read(Input::Bytes(&glb), &ReadOptions { file_name: Some("hero.glb".into()), ..Default::default() }).unwrap();
    assert_eq!(named.files()[0].name, "hero.glb");
}

#[test]
fn from_mismatch_is_format_mismatch() {
    let glb = one(&fixtures::cube(), Format::Glb);
    let e = Model::read(Input::Bytes(&glb), &ReadOptions { from: Some(Format::Fbx), ..Default::default() }).unwrap_err();
    assert_eq!(e.code, ErrorCode::FormatMismatch);
}

#[test]
fn no_model_among_inputs_is_unrecognized() {
    for bytes in [b"".as_slice(), b"hello"] {
        assert_eq!(read(Input::Bytes(bytes)).unwrap_err().code, ErrorCode::FormatUnrecognized, "{bytes:?}");
    }
    assert_eq!(read(Input::Files(&[])).unwrap_err().code, ErrorCode::FormatUnrecognized);
    let not_models = [
        InputFile { name: "readme.txt".into(), data: b"hello".as_slice().into() },
        InputFile { name: "tex.png".into(), data: fixtures::quadrant_png().into() },
    ];
    let e = read(Input::Files(&not_models)).unwrap_err();
    assert_eq!(e.code, ErrorCode::FormatUnrecognized);
    assert!(e.message().contains("readme.txt") && e.message().contains("tex.png"), "{}", e.message());
}

#[test]
fn oversize_input_is_refused_before_parsing() {
    let big = vec![0u8; MAX_INPUT_BYTES + 1];
    let e = Model::read(Input::Bytes(&big), &ReadOptions { from: Some(Format::Glb), ..Default::default() }).unwrap_err();
    assert_eq!(e.code, ErrorCode::InputTooLarge);
    let half = vec![0u8; MAX_INPUT_BYTES / 2 + 1];
    let files = [
        InputFile { name: "model.gltf".into(), data: half.as_slice().into() },
        InputFile { name: "model.bin".into(), data: half.as_slice().into() },
    ];
    assert_eq!(read(Input::Files(&files)).unwrap_err().code, ErrorCode::InputTooLarge);
}

#[test]
fn files_pick_the_model_and_give_roles() {
    let mut files = files_of(written(&fixtures::textured(), Format::Gltf, false));
    files.push(InputFile { name: "readme.txt".into(), data: b"notes".as_slice().into() });
    files.reverse();
    let m = read(Input::Files(&files)).unwrap();
    assert_eq!(m.format(), Format::Gltf);
    assert!(m.info().unwrap().textures >= 1);
    for f in &files {
        let want = match f.name.rsplit('.').next().unwrap() {
            "gltf" => FileRole::Model,
            "bin" => FileRole::Buffer,
            "png" | "jpg" => FileRole::Texture,
            "txt" => FileRole::Unused,
            other => panic!("unexpected file {other}"),
        };
        assert_eq!(role_of(&m, &f.name), want, "{}", f.name);
    }
}

#[test]
fn obj_package_roles() {
    let files = files_of(written(&fixtures::textured(), Format::Obj, false));
    let m = read(Input::Files(&files)).unwrap();
    assert_eq!(m.format(), Format::Obj);
    assert!(m.info().unwrap().textures >= 1);
    let roles: Vec<FileRole> = files.iter().map(|f| role_of(&m, &f.name)).collect();
    assert_eq!(roles[0], FileRole::Model);
    assert!(roles.contains(&FileRole::MaterialLibrary) && roles.contains(&FileRole::Texture), "{roles:?}");
}

#[test]
fn a_package_zip_unpacks() {
    let zipped = zip::write_package(&written(&fixtures::textured(), Format::Obj, false)).unwrap();
    let m = read(Input::Bytes(&zipped)).unwrap();
    assert_eq!(m.format(), Format::Obj);
    assert!(m.info().unwrap().textures >= 1);
    let roles: Vec<FileRole> = m.files().iter().map(|f| f.role).collect();
    assert!(roles.contains(&FileRole::Model) && roles.contains(&FileRole::MaterialLibrary), "{roles:?}");
}

#[test]
fn ambiguous_models() {
    let obj = one(&fixtures::cube(), Format::Obj);
    let glb = one(&fixtures::cube(), Format::Glb);
    let two_objs = [InputFile { name: "a.obj".into(), data: obj.as_slice().into() }, InputFile { name: "b.obj".into(), data: obj.as_slice().into() }];
    let e = read(Input::Files(&two_objs)).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::InputAmbiguous, Some(OptionName::FileName)));
    assert!(e.message().contains("a.obj") && e.message().contains("b.obj"), "{}", e.message());
    let as_obj = ReadOptions { from: Some(Format::Obj), ..Default::default() };
    assert_eq!(Model::read(Input::Files(&two_objs), &as_obj).unwrap_err().code, ErrorCode::InputAmbiguous);
    let named = ReadOptions { file_name: Some("b.obj".into()), ..Default::default() };
    let m = Model::read(Input::Files(&two_objs), &named).unwrap();
    assert_eq!(role_of(&m, "b.obj"), FileRole::Model);
    assert_eq!(role_of(&m, "a.obj"), FileRole::Unused);

    let mixed = [InputFile { name: "a.obj".into(), data: obj.as_slice().into() }, InputFile { name: "b.glb".into(), data: glb.into() }];
    let m = Model::read(Input::Files(&mixed), &ReadOptions { from: Some(Format::Glb), ..Default::default() }).unwrap();
    assert_eq!(m.format(), Format::Glb);
    assert_eq!(role_of(&m, "b.glb"), FileRole::Model);
}

#[test]
fn sidecar_fallback_ignores_case_and_folder() {
    // As a Windows OBJ or a flat browser drop arrives: the material library
    // and the texture renamed in case, the texture out of its folder.
    let files: Vec<InputFile> = files_of(written(&fixtures::textured(), Format::Obj, false))
        .into_iter()
        .map(|f| {
            let base = f.name.rsplit('/').next().unwrap().to_string();
            let name = if f.name.ends_with(".obj") { f.name } else { base.to_ascii_uppercase() };
            InputFile { name, data: f.data }
        })
        .collect();
    assert!(files.iter().any(|f| f.name.ends_with(".PNG")) && files.iter().any(|f| f.name.ends_with(".MTL")));
    let m = read(Input::Files(&files)).unwrap();
    assert!(m.info().unwrap().textures >= 1, "the texture resolved: {:?}", m.warnings());
    for f in files.iter().filter(|f| f.name.ends_with(".PNG")) {
        assert_eq!(role_of(&m, &f.name), FileRole::Texture);
    }
    for f in files.iter().filter(|f| f.name.ends_with(".MTL")) {
        assert_eq!(role_of(&m, &f.name), FileRole::MaterialLibrary);
    }
}

/// A gltf package whose first image is named `Textür.png`, its uri
/// percent-encoded as glTF says.
fn non_ascii_gltf() -> Vec<InputFile<'static>> {
    let mut files = files_of(written(&fixtures::textured(), Format::Gltf, false));
    let gltf = files.iter().position(|f| f.name.ends_with(".gltf")).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&files[gltf].data).unwrap();
    let old = json["images"][0]["uri"].as_str().unwrap().to_string();
    json["images"][0]["uri"] = serde_json::Value::String("Text%C3%BCr.png".into());
    files[gltf].data = serde_json::to_vec(&json).unwrap().into();
    let image = files.iter().position(|f| f.name == old).unwrap_or_else(|| panic!("{old} among the files"));
    files[image].name = "Textür.png".into();
    files
}

#[test]
fn non_ascii_sidecar_names_resolve() {
    let files = non_ascii_gltf();
    let m = read(Input::Files(&files)).unwrap();
    assert!(m.info().unwrap().textures >= 1, "{:?}", m.warnings());
    assert_eq!(role_of(&m, "Textür.png"), FileRole::Texture);

    let package = Package { files: files.into_iter().map(|f| litegen_mesh::PackageFile { path: f.name, bytes: f.data.into_owned() }).collect() };
    let zipped = zip::write_package(&package).unwrap();
    let m = read(Input::Bytes(&zipped)).unwrap();
    assert!(m.info().unwrap().textures >= 1, "{:?}", m.warnings());
    assert_eq!(role_of(&m, "Textür.png"), FileRole::Texture);
}

/// A USD layer reads under either name, whichever encoding it is in, as
/// litegen's HTTP API reads it; detection names the encoding.
#[test]
fn usd_reads_either_encoding_under_either_name() {
    let scene = fixtures::skinned();
    let text = one(&scene, Format::Usda);
    let crate_ = one(&scene, Format::Usdc);
    assert!(text.starts_with(b"#usda") && crate_.starts_with(b"PXR-USDC"));
    let want = Model::read(Input::Bytes(&text), &ReadOptions::default()).unwrap().info().unwrap().triangles;
    for (bytes, detected) in [(&text, Format::Usda), (&crate_, Format::Usdc)] {
        assert_eq!(Model::read(Input::Bytes(bytes), &ReadOptions::default()).unwrap().format(), detected);
        for from in [Format::Usda, Format::Usdc] {
            let m = Model::read(Input::Bytes(bytes), &ReadOptions { from: Some(from), ..Default::default() }).unwrap();
            assert_eq!(m.format(), from, "{detected:?} read as {from:?}");
            assert_eq!(m.info().unwrap().triangles, want, "{detected:?} read as {from:?}");
        }
    }
}

#[test]
fn read_option_rules() {
    let glb = one(&fixtures::cube(), Format::Glb);
    let e = Model::read(Input::Bytes(&glb), &ReadOptions { from_units: Some(Unit::Mm), ..Default::default() }).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(OptionName::FromUnits)));
    assert_eq!(e.message(), "`from_units` applies when `from` is stl, obj or ply, which carry no units or up axis; glb declares its own");
    let e = Model::read(Input::Bytes(&glb), &ReadOptions { fbx_color_space: Some(FbxColorSpace::Linear), ..Default::default() }).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(OptionName::FbxColorSpace)));
    assert_eq!(e.message(), "`fbx_color_space` applies when `from` is fbx");

    let obj = one(&fixtures::cube(), Format::Obj);
    Model::read(Input::Bytes(&obj), &ReadOptions { from_up: Some(UpAxis::Z), ..Default::default() }).unwrap();
    let fbx = one(&fixtures::cube(), Format::Fbx);
    Model::read(Input::Bytes(&fbx), &ReadOptions { fbx_color_space: Some(FbxColorSpace::Linear), ..Default::default() }).unwrap();

    let e = Model::read(Input::Bytes(&glb), &ReadOptions { fbx_light_units: Some(LightUnits::Blender), ..Default::default() }).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(OptionName::FbxLightUnits)));
    assert_eq!(e.message(), "`fbx_light_units` applies when `from` is fbx");
    let e = Model::read(Input::Bytes(&fbx), &ReadOptions { usd_light_units: Some(LightUnits::Blender), ..Default::default() }).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(OptionName::UsdLightUnits)));
    assert_eq!(e.message(), "`usd_light_units` applies when `from` is usdz, usda or usdc");
    Model::read(Input::Bytes(&fbx), &ReadOptions { fbx_light_units: Some(LightUnits::Blender), ..Default::default() }).unwrap();
    for f in [Format::Usdz, Format::Usda, Format::Usdc] {
        let usd = one(&fixtures::cube(), f);
        Model::read(Input::Bytes(&usd), &ReadOptions { usd_light_units: Some(LightUnits::Blender), ..Default::default() }).unwrap();
    }
}

fn extent(m: &Model) -> [f64; 3] {
    let b = m.info().unwrap().bounds.as_ref().unwrap();
    [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]]
}

#[test]
fn from_units_and_up_set_the_frame() {
    // A box longer in z than in y, so turning z up shows.
    let stl = b"solid box\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 2 0 0\n   vertex 0 1 4\n  endloop\n endfacet\nendsolid box\n";
    let plain = read(Input::Bytes(stl)).unwrap();
    assert_eq!(extent(&plain), [2.0, 1.0, 4.0]);
    let mm = Model::read(Input::Bytes(stl), &ReadOptions { from_units: Some(Unit::Mm), ..Default::default() }).unwrap();
    let e = extent(&mm);
    for (got, want) in e.iter().zip([0.002, 0.001, 0.004]) {
        assert!((got - want).abs() < 1e-9, "{e:?}");
    }
    let z_up = Model::read(Input::Bytes(stl), &ReadOptions { from_up: Some(UpAxis::Z), ..Default::default() }).unwrap();
    let e = extent(&z_up);
    assert!((e[1] - 4.0).abs() < 1e-6 && (e[2] - 1.0).abs() < 1e-6, "+Z up becomes +Y up: {e:?}");
}

/// `info` against mesh's own `inspect` of the same file, field by field.
fn assert_info_is_inspects(data: &[u8], format: Format) -> litegen_3d_converter::ModelInfo {
    let m = read(Input::Bytes(data)).unwrap();
    let scene = litegen_mesh::read_scene(data, MeshFormat::parse(format.as_str()).unwrap(), &litegen_mesh::NoSidecars).unwrap().scene;
    let f = inspect::inspect(&scene).unwrap();
    let i = m.info().unwrap().clone();
    assert_eq!(
        (i.triangles as usize, i.vertices as usize, i.uv_sets as usize, i.vertex_colors),
        (f.triangle_count, f.vertex_count, f.uv_sets, f.vertex_colors)
    );
    assert_eq!((i.nodes as usize, i.meshes as usize, i.textures as usize), (scene.nodes.len(), scene.meshes.len(), scene.images.len()));
    assert_eq!(i.bounds.map(|b| (b.min, b.max)), f.bounds.map(|(lo, hi)| (lo.map(f64::from), hi.map(f64::from))));
    let maps: Vec<(Option<String>, Vec<&str>)> = i.materials.iter().map(|m| (m.name.clone(), m.maps.iter().map(|t| t.as_str()).collect())).collect();
    let want: Vec<(Option<String>, Vec<&str>)> = f.materials.iter().map(|m| (m.name.clone(), m.maps.clone())).collect();
    assert_eq!(maps, want);
    match (&i.skin, &f.skin) {
        (Some(s), Some(fs)) => {
            assert_eq!((s.joints as usize, s.root.as_deref(), s.bind_pose.as_str()), (fs.joints.len(), fs.root.as_deref(), fs.bind_pose.as_str()));
            let humanoid = s.humanoid.as_ref().map(|h| (h.naming.as_str(), h.bones.iter().map(|b| (b.bone.as_str().to_string(), b.joint.clone())).collect::<Vec<_>>()));
            let want = fs.humanoid.as_ref().map(|(table, map)| (*table, map.clone()));
            assert_eq!(humanoid, want);
        }
        (None, None) => {}
        (got, want) => panic!("skin {got:?} vs inspect's {want:?}"),
    }
    let clips: Vec<_> = i.animations.iter().map(|a| (a.name.clone(), a.duration_s, a.root_travel_m, a.in_place)).collect();
    let want: Vec<_> =
        f.animations.iter().map(|c| (c.name.clone(), f64::from(c.duration_s), c.root_travel_m.map(f64::from), c.in_place())).collect();
    assert_eq!(clips, want);
    i
}

macro_rules! testdata {
    ($path:literal) => {
        include_bytes!(concat!("../../../../litegen-core/src/mesh/testdata/", $path)).as_slice()
    };
}

#[test]
fn info_matches_inspect() {
    // Files picked for values other than the defaults, so a swapped or
    // constant field shows.
    let skinned = assert_info_is_inspects(&one(&fixtures::skinned(), Format::Glb), Format::Glb);
    assert!(skinned.skin.is_some() && !skinned.animations.is_empty());
    let travelling = assert_info_is_inspects(testdata!("gltf/gltfpack/skinned.glb"), Format::Glb);
    assert!(travelling.animations.iter().any(|a| !a.in_place && a.root_travel_m != [0.0; 3]), "{:?}", travelling.animations);
    assert_ne!(travelling.triangles, travelling.vertices);
    let multiuv = assert_info_is_inspects(testdata!("gltf/gltfpack/multiuv.glb"), Format::Glb);
    assert_eq!(multiuv.uv_sets, 4);
    let coloured = assert_info_is_inspects(testdata!("fbx/textured_assimp_7500.fbx"), Format::Fbx);
    assert!(coloured.vertex_colors && coloured.nodes != coloured.meshes);
    let humanoid = assert_info_is_inspects(HUMANOID_MOCK, Format::Gltf);
    assert!(humanoid.skin.as_ref().is_some_and(|s| s.humanoid.is_some()));
}

#[test]
fn reader_warnings_carry_through() {
    let obj = b"mtllib missing.mtl\nusemtl red\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let m = read(Input::Bytes(obj)).unwrap();
    assert!(!m.warnings().is_empty(), "the missing .mtl is named");
    assert!(m.warnings().iter().any(|w| w.contains("missing.mtl")), "{:?}", m.warnings());
}

#[test]
fn a_mac_made_zip_ignores_finders_files() {
    // Finder's "Compress" adds AppleDouble copies (`__MACOSX/._model.obj`)
    // and .DS_Store, which would otherwise look like a second model.
    let mut package = written(&fixtures::textured(), Format::Obj, false);
    let obj = package.files[0].path.clone();
    package.files.push(litegen_mesh::PackageFile { path: format!("__MACOSX/._{obj}"), bytes: vec![0, 5, 22, 7, 0, 2] });
    package.files.push(litegen_mesh::PackageFile { path: ".DS_Store".into(), bytes: vec![0, 0, 0, 1] });
    let m = read(Input::Bytes(&zip::write_package(&package).unwrap())).unwrap();
    assert_eq!(m.format(), Format::Obj);
    assert!(m.files().iter().all(|f| !f.name.contains("__MACOSX") && f.name != ".DS_Store"), "{:?}", m.files());
}

#[test]
fn a_lone_zip_may_carry_its_own_name() {
    let zipped = zip::write_package(&written(&fixtures::textured(), Format::Obj, false)).unwrap();
    let named = |name: &str| ReadOptions { file_name: Some(name.into()), ..Default::default() };
    assert_eq!(Model::read(Input::Bytes(&zipped), &named("fox.zip")).unwrap().format(), Format::Obj);
    let one_file = [InputFile { name: "fox.zip".into(), data: zipped.as_slice().into() }];
    assert_eq!(Model::read(Input::Files(&one_file), &named("fox.zip")).unwrap().format(), Format::Obj);
    // A name inside the zip still says which file is the model.
    let m = Model::read(Input::Files(&one_file), &named("model.obj")).unwrap();
    assert_eq!(role_of(&m, "model.obj"), FileRole::Model);
    let e = Model::read(Input::Files(&one_file), &named("other.zip")).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamInvalid, Some(OptionName::FileName)));
}

/// A GLB of one mesh of `triangles` unconnected triangles, placed by
/// `instances` nodes.
fn instanced_glb(instances: usize, triangles: usize) -> Vec<u8> {
    let n = triangles * 3;
    let mut bin = Vec::with_capacity(n * 16);
    for k in 0..triangles {
        let (x, y) = ((k % 200) as f32 * 0.01, (k / 200) as f32 * 0.01);
        for v in [x, y, 0.0, x + 0.005, y, 0.0, x, y + 0.005, 0.0] {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    let positions = bin.len();
    for i in 0..n as u32 {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let json = serde_json::json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": (0..instances).collect::<Vec<_>>() }],
        "nodes": (0..instances).map(|i| serde_json::json!({ "mesh": 0, "translation": [i as f32 * 3.0, 0.0, 0.0] })).collect::<Vec<_>>(),
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1 }] }],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": n, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [2.0, 1.005, 0.0] },
            { "bufferView": 1, "componentType": 5125, "count": n, "type": "SCALAR" },
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": positions },
            { "buffer": 0, "byteOffset": positions, "byteLength": bin.len() - positions },
        ],
        "buffers": [{ "byteLength": bin.len() }],
    });
    let mut text = serde_json::to_vec(&json).unwrap();
    text.resize(text.len().div_ceil(4) * 4, b' ');
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    for word in [2, total as u32, text.len() as u32, 0x4E4F_534A] {
        glb.extend_from_slice(&u32::to_le_bytes(word));
    }
    glb.extend_from_slice(&text);
    for word in [bin.len() as u32, 0x004E_4942] {
        glb.extend_from_slice(&u32::to_le_bytes(word));
    }
    glb.extend_from_slice(&bin);
    glb
}

#[test]
fn reading_and_converting_never_measure_the_model() {
    // 70 nodes place one 60,000-vertex mesh: 4.2 million vertices once every
    // instance is placed, past what measuring flattens (4 million). The
    // hosted converter, which never measures, converts it.
    let glb = instanced_glb(70, 20_000);
    let to = |f: Format| ConvertWriteOptions::new(f);
    assert_eq!(convert(Input::Bytes(&glb), &ReadOptions::default(), &to(Format::Glb)).unwrap().format, Format::Glb);
    let model = read(Input::Bytes(&glb)).unwrap();
    assert_eq!(model.write(&to(Format::Gltf)).unwrap().format, Format::Gltf);
    assert_eq!(model.info().unwrap_err().code, ErrorCode::MeshUnsupported);
    let small = read(Input::Bytes(&instanced_glb(2, 10))).unwrap();
    assert_eq!(small.info().unwrap().triangles, 10, "each mesh counted once, however many nodes place it");
}

#[test]
fn bindings_check_the_cap_by_the_cores_rule() {
    assert!(check_input_size(MAX_INPUT_BYTES).is_ok());
    let e = check_input_size(MAX_INPUT_BYTES + 1).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::InputTooLarge, None));
    assert_eq!(e.message(), read(Input::Bytes(&vec![0u8; MAX_INPUT_BYTES + 1])).unwrap_err().message());
}

#[test]
fn material_maps_are_the_slots_inspect_names() {
    // The fixtures, and `layered` with every slot it has a layer for set, so
    // all 17 maps are compared.
    let mut every = fixtures::layered();
    let r = every.materials.iter().find_map(|m| m.texture_refs().next().map(|(_, r)| r.clone())).expect("layered has a texture");
    for m in &mut every.materials {
        m.normal_texture = Some(r.clone());
        m.emissive_texture = Some(r.clone());
        if let Some(t) = m.transmission.as_mut() {
            t.texture = Some(r.clone());
        }
        if let Some(s) = m.specular.as_mut() {
            s.texture = Some(r.clone());
        }
        if let Some(s) = m.sheen.as_mut() {
            s.roughness_texture = Some(r.clone());
        }
        if let Some(i) = m.iridescence.as_mut() {
            i.texture = Some(r.clone());
        }
        if let Some(a) = m.anisotropy.as_mut() {
            a.texture = Some(r.clone());
        }
    }
    let mut seen = BTreeSet::new();
    for (name, scene) in fixtures::all().into_iter().chain([("layered, every slot", every)]) {
        let glb = one(&scene, Format::Glb);
        let m = read(Input::Bytes(&glb)).unwrap();
        let ours: Vec<Vec<&str>> = m.info().unwrap().materials.iter().map(|mat| mat.maps.iter().map(|t| t.as_str()).collect()).collect();
        let read_back = litegen_mesh::read_scene(&glb, MeshFormat::Glb, &litegen_mesh::NoSidecars).unwrap().scene;
        let theirs: Vec<Vec<&str>> = inspect::inspect(&read_back).unwrap().materials.into_iter().map(|mat| mat.maps).collect();
        assert_eq!(ours, theirs, "{name}");
        seen.extend(m.info().unwrap().materials.iter().flat_map(|mat| mat.maps.clone()));
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), TextureMap::ALL.to_vec(), "every map is compared");
}

/// Seven joints named as litegen's mock provider names a humanoid's.
const HUMANOID_MOCK: &[u8] = include_bytes!("../../testdata/humanoid_mock.gltf");

#[test]
fn a_humanoid_rig_names_its_bones() {
    let m = read(Input::Bytes(HUMANOID_MOCK)).unwrap();
    let humanoid = m.info().unwrap().skin.as_ref().unwrap().humanoid.clone().unwrap();
    assert_eq!(humanoid.naming, HumanoidNaming::LitegenMock);
    let bones: Vec<(VrmBone, &str)> = humanoid.bones.iter().map(|b| (b.bone, b.joint.as_str())).collect();
    use VrmBone::*;
    assert_eq!(
        bones,
        [(Hips, "hips"), (Spine, "torso"), (Head, "head"), (LeftUpperArm, "arm_l"), (RightUpperArm, "arm_r"), (LeftUpperLeg, "leg_l"), (RightUpperLeg, "leg_r")]
    );
    let plain = read(Input::Bytes(&one(&fixtures::skinned(), Format::Glb))).unwrap();
    assert_eq!(plain.info().unwrap().skin.as_ref().unwrap().humanoid, None);
}

/// mesh names its humanoid tables and bones as strings (humanoid.rs); the
/// typed sets must list exactly those, so a table or bone mesh adds fails
/// here until it is typed.
#[test]
fn every_humanoid_table_and_bone_of_mesh_is_typed() {
    let source = include_str!("../../../../litegen-core/src/mesh/humanoid.rs");
    let tables = &source[source.find("const TABLES").unwrap()..];
    let tables = &tables[..tables.find("];").unwrap()];
    let names: Vec<&str> = tables.lines().filter_map(|l| l.trim().strip_prefix("(\"")?.split('"').next()).collect();
    assert_eq!(names, HumanoidNaming::ALL.map(HumanoidNaming::as_str));
    // Every `("bone", "joint")` pair of every table (all defined before TABLES).
    let mut bones = BTreeSet::new();
    let mut rest = &source[source.find("const VRM: Table").unwrap()..source.find("const TABLES").unwrap()];
    while let Some(at) = rest.find("(\"") {
        rest = &rest[at + 2..];
        let Some((bone, after)) = rest.split_once('"') else { break };
        if after.starts_with(", \"") {
            bones.insert(bone);
        }
    }
    let typed: BTreeSet<&str> = VrmBone::ALL.iter().map(|b| b.as_str()).collect();
    assert_eq!(bones, typed);
    for t in TextureMap::ALL {
        assert_eq!(TextureMap::ALL.iter().filter(|o| o.as_str() == t.as_str()).count(), 1, "{t:?}");
    }
}

/// Review Focus 2: a sidecar's name may reach the converter in another Unicode
/// form than the model's reference (macOS and its zips decompose, exporters
/// compose) or in another case, beyond ASCII.
#[test]
fn sidecar_names_match_across_unicode_forms_and_case() {
    // glTF: the buffer `モデル.bin`, referenced composed (NFC) and
    // percent-encoded, given decomposed (NFD: デ is テ + U+3099).
    let mut files = files_of(written(&fixtures::textured(), Format::Gltf, false));
    let gltf = files.iter().position(|f| f.name.ends_with(".gltf")).unwrap();
    let mut json: serde_json::Value = serde_json::from_slice(&files[gltf].data).unwrap();
    let old_bin = json["buffers"][0]["uri"].as_str().unwrap().to_string();
    json["buffers"][0]["uri"] = serde_json::Value::String("%E3%83%A2%E3%83%87%E3%83%AB.bin".into());
    files[gltf].data = serde_json::to_vec(&json).unwrap().into();
    let bin = files.iter().position(|f| f.name == old_bin).unwrap();
    files[bin].name = "モテ\u{3099}ル.bin".into();
    let m = read(Input::Files(&files)).unwrap();
    assert_eq!(role_of(&m, "モテ\u{3099}ル.bin"), FileRole::Buffer);

    // OBJ: the texture referenced as `Textür.png` (NFC), given decomposed,
    // then given in upper case.
    let package = written(&fixtures::textured(), Format::Obj, false);
    for given in ["Textu\u{308}r.png", "TEXTÜR.PNG"] {
        let mut files = files_of(package.clone());
        let mtl = files.iter().position(|f| f.name.ends_with(".mtl")).unwrap();
        let text = String::from_utf8(files[mtl].data.to_vec()).unwrap();
        let png = files.iter().position(|f| f.name.ends_with(".png")).unwrap();
        let old = files[png].name.clone();
        files[mtl].data = text.replace(&old, "Textür.png").into_bytes().into();
        files[png].name = given.into();
        let m = read(Input::Files(&files)).unwrap();
        assert_eq!(role_of(&m, given), FileRole::Texture, "{given}: {:?}", m.warnings());
    }
}

/// Review Focus 4: an empty file is no format at all, whatever its name says.
#[test]
fn an_empty_named_file_is_unrecognized() {
    for name in ["model.obj", "model.glb", "model.stl", "model.fbx", "model.usda"] {
        let files = [InputFile { name: name.into(), data: b"".as_slice().into() }];
        assert_eq!(read(Input::Files(&files)).unwrap_err().code, ErrorCode::FormatUnrecognized, "{name}");
        let named = ReadOptions { file_name: Some(name.into()), ..Default::default() };
        assert_eq!(Model::read(Input::Bytes(b""), &named).unwrap_err().code, ErrorCode::FormatUnrecognized, "{name}");
    }
}

#[test]
fn a_file_that_is_not_from_names_from_as_the_api_does() {
    let glb = one(&fixtures::cube(), Format::Glb);
    let e = Model::read(Input::Bytes(&glb), &ReadOptions { from: Some(Format::Fbx), ..Default::default() }).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::FormatMismatch, Some(OptionName::From)));
    assert!(e.message().ends_with("; check that `from` (fbx) matches the file"), "{}", e.message());
}

#[test]
fn from_matching_no_file_says_so() {
    let files = [
        InputFile { name: "a.obj".into(), data: one(&fixtures::cube(), Format::Obj).into() },
        InputFile { name: "b.png".into(), data: fixtures::quadrant_png().into() },
    ];
    let e = Model::read(Input::Files(&files), &ReadOptions { from: Some(Format::Fbx), ..Default::default() }).unwrap_err();
    assert_eq!(e.code, ErrorCode::FormatUnrecognized);
    assert!(e.message().starts_with("no file among the files (a.obj, b.png) is a fbx file"), "{}", e.message());
}

#[test]
fn convert_checks_its_write_options_before_parsing() {
    // A broken GLB and an option glb does not take: the option is the answer,
    // before any parsing.
    let broken = b"glTF\x02\x00\x00\x00\xff\xff\xff\xff not a gltf".to_vec();
    let read = ReadOptions { from: Some(Format::Glb), ..Default::default() };
    let write = ConvertWriteOptions { units: Some(Unit::Mm), ..ConvertWriteOptions::new(Format::Glb) };
    let e = convert(Input::Bytes(&broken), &read, &write).unwrap_err();
    assert_eq!((e.code, e.param), (ErrorCode::ParamUnsupported, Some(OptionName::Units)));
}
