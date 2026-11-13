//! Detection: magic bytes first, then the file name, then weak content
//! heuristics, on our own writer's files and on files other tools wrote.

use litegen_3d_converter::{detect_format, Format};
use litegen_mesh::{fixtures, package::Package, write_scene, zip, MeshFormat, WriteOptions};

macro_rules! testdata {
    ($path:literal) => {
        include_bytes!(concat!("../../../../litegen-core/src/mesh/testdata/", $path)).as_slice()
    };
}

fn written(scene: &litegen_mesh::Scene, f: Format, self_contained: bool) -> Package {
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

#[test]
fn detects_every_written_format_without_a_name() {
    for (name, scene) in [("textured", fixtures::textured()), ("skinned", fixtures::skinned())] {
        for f in Format::ALL {
            let file = written(&scene, f, true).files.remove(0);
            assert_eq!(detect_format(&file.bytes, None), Some(f), "{name} written as {f:?}");
        }
    }
}

#[test]
fn detects_independent_tools_files() {
    let cases: [(&[u8], Format); 8] = [
        (testdata!("fbx/textured_assimp_7500.fbx"), Format::Fbx),
        (testdata!("fbx/cube_assimp_ascii.fbx"), Format::Fbx),
        (testdata!("dae/scenekit/skinned_scenekit.dae"), Format::Dae),
        (testdata!("3mf/assimp_textured.3mf"), Format::ThreeMf),
        (testdata!("usd/textured_apple.usdz"), Format::Usdz),
        (testdata!("usd/cube.usdc"), Format::Usdc),
        (testdata!("usd/comp.usda"), Format::Usda),
        (testdata!("ply/cube_be_struct.ply"), Format::Ply),
    ];
    for (i, (bytes, want)) in cases.into_iter().enumerate() {
        assert_eq!(detect_format(bytes, None), Some(want), "case {i}");
    }
}

#[test]
fn magic_beats_the_name() {
    let glb = written(&fixtures::cube(), Format::Glb, true).files.remove(0).bytes;
    assert_eq!(detect_format(&glb, Some("x.obj")), Some(Format::Glb));
    let fbx = testdata!("fbx/textured_assimp_7500.fbx");
    assert_eq!(detect_format(fbx, Some("model.stl")), Some(Format::Fbx));
    // A `.usd` is either encoding: its bytes say which.
    assert_eq!(detect_format(testdata!("usd/cube.usdc"), Some("scene.usd")), Some(Format::Usdc));
    assert_eq!(detect_format(testdata!("usd/comp.usda"), Some("scene.usd")), Some(Format::Usda));
    assert_eq!(detect_format(testdata!("usd/comp.usda"), Some("scene.usdc")), Some(Format::Usda));
}

#[test]
fn name_decides_when_bytes_cannot() {
    assert_eq!(detect_format(b"", Some("m.usdc")), Some(Format::Usdc));
    assert_eq!(detect_format(b"", Some("scene.USD")), Some(Format::Usda));
    assert_eq!(detect_format(b"hello", Some("A.STL")), Some(Format::Stl));
    assert_eq!(detect_format(b"hello", Some("dir/Model.Gltf")), Some(Format::Gltf));
    assert_eq!(detect_format(b"hello", Some("notes.txt")), None);
    assert_eq!(detect_format(b"hello", Some("no-extension")), None);
}

#[test]
fn unknown_is_none() {
    assert_eq!(detect_format(b"hello world", None), None);
    assert_eq!(detect_format(br#"{"a": 1}"#, None), None);
    assert_eq!(detect_format(b"", None), None);
    let mut w = zip::Writer::new();
    w.add("readme.txt", b"not a model", zip::Store::Deflated).unwrap();
    assert_eq!(detect_format(&w.finish(), None), None);
}

#[test]
fn a_package_zip_is_not_a_model_format() {
    let package = written(&fixtures::textured(), Format::Obj, false);
    assert!(package.files.len() > 1, "an obj package has sidecars");
    let zipped = zip::write_package(&package).unwrap();
    assert_eq!(detect_format(&zipped, None), None);
    // Even named as a container, a zip holds what it holds.
    assert_eq!(detect_format(&zipped, Some("model.obj")), None);
}

#[test]
fn a_binary_stl_whose_header_says_solid_is_still_binary() {
    let mut stl = written(&fixtures::cube(), Format::Stl, true).files.remove(0).bytes;
    stl[..5].copy_from_slice(b"solid");
    assert_eq!(detect_format(&stl, None), Some(Format::Stl));
}

#[test]
fn ascii_stl_and_obj_by_content() {
    let ascii_stl = b"solid cube\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 0\n  endloop\n endfacet\nendsolid cube\n";
    assert_eq!(detect_format(ascii_stl, None), Some(Format::Stl));
    let obj = b"# a triangle\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    assert_eq!(detect_format(obj, None), Some(Format::Obj));
    // A file of vertices with no faces is not a mesh we recognise.
    assert_eq!(detect_format(b"v 0 0 0\nv 1 0 0\n", None), None);
}

#[test]
fn a_gltf_whose_asset_key_comes_late_is_still_gltf() {
    // A self-contained .gltf can carry its buffers as data URIs before `asset`.
    let uri = "A".repeat(200_000);
    let json = format!(r#"{{"buffers":[{{"uri":"data:application/octet-stream;base64,{uri}"}}],"asset":{{"version":"2.0"}}}}"#);
    assert_eq!(detect_format(json.as_bytes(), None), Some(Format::Gltf));
}

#[test]
fn an_obj_whose_first_face_is_past_the_sniff_window_is_obj() {
    // As exporters write OBJ: every vertex first, then the faces.
    let mut obj = String::new();
    for i in 0..4000 {
        obj.push_str(&format!("v {i}.000000 0.000000 0.000000\n"));
    }
    obj.push_str("f 1 2 3\n");
    assert!(obj.len() > 64 * 1024);
    assert_eq!(detect_format(obj.as_bytes(), None), Some(Format::Obj));
    // Vertices alone are not a model a reader would take.
    assert_eq!(detect_format(obj.split("f 1").next().unwrap().as_bytes(), None), None);
}
