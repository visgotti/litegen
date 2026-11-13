//! HTTP coverage for the standalone mesh converter, `/v1/utils/models3d/*`.
//!
//! The endpoint is a thin wrapper, so the oracle is `litegen::mesh::convert`
//! itself: whatever the library produces for a pair, the endpoint must return
//! byte for byte. That keeps these tests true as the converter grows formats.

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use litegen::mesh::{self, MeshFormat};
use litegen::proxy::model3d_convert::derivable_formats;
use tower::ServiceExt;

fn cube_glb() -> Vec<u8> {
    litegen::providers::model3d::glb::generate_cube_glb("a cube")
}

/// Posts to the converter. The tests in this binary run at once and share the
/// process-wide conversion slots, and a slot is held until its file is read,
/// so a busy moment answers 503 `server_at_capacity`. That is retried here, as
/// its `Retry-After` asks a client to.
async fn convert(app: &Router, query: &str, body: Vec<u8>) -> axum::response::Response {
    post_convert(app, query, || Body::from(body.clone())).await
}

async fn post_convert(app: &Router, query: &str, body: impl Fn() -> Body) -> axum::response::Response {
    for _ in 0..1000 {
        let resp = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/utils/models3d/convert{query}"))
                    .header("content-type", "application/octet-stream")
                    .body(body())
                    .unwrap(),
            )
            .await
            .unwrap();
        if resp.status() != StatusCode::SERVICE_UNAVAILABLE {
            return resp;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the converter stayed at capacity for 10 s")
}

async fn body_bytes(resp: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), 256 * 1024 * 1024).await.unwrap().to_vec()
}

#[tokio::test]
async fn every_pair_matches_the_library_byte_for_byte() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = cube_glb();
    for from in derivable_formats() {
        let from_fmt = MeshFormat::parse(&from).unwrap();
        let source = mesh::convert(&glb, MeshFormat::Glb, from_fmt).unwrap();
        for to in derivable_formats() {
            let to_fmt = MeshFormat::parse(&to).unwrap();
            let resp = convert(&app, &format!("?from={from}&to={to}"), source.clone()).await;
            assert_eq!(resp.status(), StatusCode::OK, "{from} -> {to}");
            let ct = resp.headers()["content-type"].to_str().unwrap().to_string();
            assert_eq!(
                ct,
                litegen::proxy::storage::model3d_content_type(&to, "application/octet-stream"),
                "{from} -> {to}"
            );
            let cd = resp.headers()["content-disposition"].to_str().unwrap().to_string();
            assert!(cd.ends_with(&format!(".{to}\"")), "{from} -> {to}: {cd}");
            let got = body_bytes(resp).await;
            let want = mesh::convert(&source, from_fmt, to_fmt).unwrap();
            assert_eq!(got, want, "{from} -> {to}: endpoint and library disagree");
        }
    }
}

#[tokio::test]
async fn format_names_are_case_insensitive() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = convert(&app, "?from=GLB&to=Obj", cube_glb()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "model/obj");
}

#[tokio::test]
async fn a_missing_format_names_the_param_and_the_supported_set() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (query, param) in [("?to=obj", "from"), ("?from=glb", "to"), ("", "from"), ("?from=&to=obj", "from")] {
        let resp = convert(&app, query, cube_glb()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query:?}");
        let body = harness::json_body(resp).await;
        assert_eq!(body["error"]["code"], "param_required", "{query:?}: {body}");
        assert_eq!(body["error"]["param"], param, "{query:?}: {body}");
        let msg = body["error"]["message"].as_str().unwrap();
        for f in derivable_formats() {
            assert!(msg.contains(&f), "{query:?}: message should list {f}: {msg}");
        }
    }
}

#[tokio::test]
async fn a_format_the_converter_cannot_handle_is_a_400_not_a_guess() {
    let (app, _db) = harness::app_with_mock_3d().await;
    // `blend` is outside every vocabulary litegen has; whatever the converter
    // grows into, it is not one of these.
    for (query, param) in [("?from=glb&to=blend", "to"), ("?from=blend&to=glb", "from")] {
        let resp = convert(&app, query, cube_glb()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = harness::json_body(resp).await;
        assert_eq!(body["error"]["code"], "format_unsupported", "{body}");
        assert_eq!(body["error"]["param"], param, "{body}");
    }
}

#[tokio::test]
async fn bytes_that_are_not_the_named_format_point_at_from() {
    let (app, _db) = harness::app_with_mock_3d().await;
    // An OBJ declared as GLB: the caller named the wrong format.
    let obj = mesh::convert(&cube_glb(), MeshFormat::Glb, MeshFormat::Obj).unwrap();
    let resp = convert(&app, "?from=glb&to=stl", obj).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = harness::json_body(resp).await;
    assert_eq!(body["error"]["code"], "format_mismatch", "{body}");
    assert_eq!(body["error"]["param"], "from", "{body}");
}

#[tokio::test]
async fn a_binary_file_declared_as_a_text_format_points_at_from() {
    // PLY's magic is checked before its header is decoded as text, so a GLB
    // sent as `from=ply` is the wrong format, not a malformed PLY.
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = convert(&app, "?from=ply&to=obj", cube_glb()).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = harness::json_body(resp).await;
    assert_eq!(body["error"]["code"], "format_mismatch", "{body}");
    assert_eq!(body["error"]["param"], "from", "{body}");
}

/// The header the endpoint must send for `losses`: kinds only, alphabetical,
/// deduplicated, no spaces; `None` when there are none.
fn expected_losses_header(losses: &[mesh::Loss]) -> Option<String> {
    let kinds: std::collections::BTreeSet<&str> = losses.iter().map(|l| l.kind.as_str()).collect();
    (!kinds.is_empty()).then(|| kinds.into_iter().collect::<Vec<_>>().join(","))
}

#[tokio::test]
async fn the_losses_header_names_exactly_what_the_library_reports() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let mut saw_loss = false;
    let mut saw_lossless = false;
    // Every fixture as GLB, which holds all of it, then out to every target:
    // rigged, morphing and textured scenes against containers that cannot hold
    // those features, which is where losses come from.
    for (name, scene) in mesh::fixtures::all() {
        let glb = mesh::write_scene(&scene, MeshFormat::Glb, mesh::WriteOptions::default())
            .unwrap()
            .package
            .files
            .remove(0)
            .bytes;
        for to in derivable_formats() {
            let to_fmt = MeshFormat::parse(&to).unwrap();
            let want = mesh::convert_scene(&glb, MeshFormat::Glb, &mesh::NoSidecars, to_fmt, mesh::WriteOptions { self_contained: true, ..Default::default() })
                .unwrap();
            let resp = convert(&app, &format!("?from=glb&to={to}"), glb.clone()).await;
            assert_eq!(resp.status(), StatusCode::OK, "{name} -> {to}");
            let got = resp
                .headers()
                .get("x-litegen-conversion-losses")
                .map(|v| v.to_str().unwrap().to_string());
            assert_eq!(got, expected_losses_header(&want.losses), "{name} -> {to}");
            assert!(!got.as_deref().unwrap_or_default().contains(' '), "{name} -> {to}: {got:?}");
            saw_loss |= got.is_some();
            saw_lossless |= got.is_none();
            assert_eq!(body_bytes(resp).await, want.package.files[0].bytes, "{name} -> {to}");
        }
    }
    assert!(saw_loss, "no fixture lost anything anywhere, so the header was never exercised");
    assert!(saw_lossless, "every conversion lost something, so the absent case was never exercised");
}

#[tokio::test]
async fn a_rigged_animated_model_to_stl_reports_what_stl_cannot_hold() {
    // The case a caller most needs warning about: STL keeps the bent pose and
    // nothing that made it move.
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = mesh::write_scene(&mesh::fixtures::skinned(), MeshFormat::Glb, mesh::WriteOptions::default())
        .unwrap()
        .package
        .files
        .remove(0)
        .bytes;
    let resp = convert(&app, "?from=glb&to=stl", glb).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let header = resp.headers()["x-litegen-conversion-losses"].to_str().unwrap().to_string();
    let kinds: Vec<&str> = header.split(',').collect();
    for k in ["animation", "skins"] {
        assert!(kinds.contains(&k), "{k} missing from {header}");
    }
    let mut sorted = kinds.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(kinds, sorted, "sorted and deduplicated");
}

#[tokio::test]
async fn hostile_or_broken_input_is_a_client_error_never_a_5xx() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = cube_glb();
    let mut cases: Vec<(String, Vec<u8>)> = vec![
        ("empty body".into(), Vec::new()),
        ("glb magic only".into(), b"glTF".to_vec()),
        ("garbage".into(), vec![0xAB; 4096]),
    ];
    for cut in [12, 20, glb.len() / 2, glb.len() - 1] {
        cases.push((format!("glb truncated to {cut}"), glb[..cut].to_vec()));
    }
    for (name, bytes) in cases {
        for from in derivable_formats() {
            let resp = convert(&app, &format!("?from={from}&to=glb"), bytes.clone()).await;
            let status = resp.status();
            assert!(status.is_client_error(), "{name} as {from}: got {status}");
            let body = harness::json_body(resp).await;
            let code = body["error"]["code"].as_str().unwrap_or_default().to_string();
            assert!(
                ["format_mismatch", "mesh_malformed", "mesh_unsupported", "mesh_empty"].contains(&code.as_str()),
                "{name} as {from}: unexpected code {code:?}: {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_body_over_the_cap_is_413_before_any_parsing() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = convert(&app, "?from=glb&to=obj", vec![0u8; mesh::MAX_INPUT_BYTES + 1]).await;
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = harness::json_body(resp).await;
    assert_eq!(body["error"]["code"], "body_too_large", "{body}");
}

#[tokio::test]
async fn converting_creates_no_generation() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = convert(&app, "?from=glb&to=stl", cube_glb()).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .clone()
        .oneshot(Request::get("/v1/generations").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = harness::json_body(resp).await;
    assert_eq!(body["total"], 0, "a conversion is not a generation: {body}");
}

#[tokio::test]
async fn formats_lists_exactly_what_the_converter_does() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app
        .clone()
        .oneshot(Request::get("/v1/utils/models3d/formats").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = harness::json_body(resp).await;
    let formats: Vec<String> = serde_json::from_value(body["formats"].clone()).unwrap();
    assert_eq!(formats, derivable_formats());
    assert_eq!(body["max_input_bytes"], mesh::MAX_INPUT_BYTES as u64);
}

#[tokio::test]
async fn both_routes_are_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app
        .clone()
        .oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let spec = harness::json_body(resp).await;
    let post = &spec["paths"]["/v1/utils/models3d/convert"]["post"];
    assert!(post.is_object(), "convert missing from spec");
    assert!(spec["paths"]["/v1/utils/models3d/formats"]["get"].is_object(), "formats missing from spec");
    // A file, not an integer array, or the SDKs type the upload as `number[]`.
    for schema in [
        &post["requestBody"]["content"]["application/octet-stream"]["schema"],
        &post["responses"]["200"]["content"]["application/octet-stream"]["schema"],
    ] {
        assert_eq!(schema["type"], "string", "{schema}");
        assert_eq!(schema["format"], "binary", "{schema}");
    }
    assert!(
        post["responses"]["200"]["headers"]["X-Litegen-Conversion-Losses"].is_object(),
        "the losses header must be documented on the 200"
    );
}

/// A generation request for the mock image model.
async fn generate(app: &Router) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post("/v1/images/generations")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({ "model": "mock/image-gen", "prompt": "a lighthouse" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

/// An upload that never finishes: a client sending slowly, or not at all.
fn stalled_upload() -> Body {
    Body::from_stream(futures::stream::pending::<Result<bytes::Bytes, std::io::Error>>())
}

/// 2026-10-31 audit: a conversion took one of the generation endpoints'
/// in-flight slots (`max_in_flight`, shared by every tenant) before reading
/// its body, with no timeout, so slow or large uploads — unbilled — could
/// answer every tenant's generations with 503. Conversions have slots of
/// their own.
#[tokio::test]
async fn a_conversion_never_takes_a_generation_slot() {
    let assets = tempfile::tempdir().unwrap();
    let (app, _db) = harness::app_with(assets.path(), |c| c.backpressure.max_in_flight = 1).await;
    let held = app.clone();
    let upload = tokio::spawn(async move {
        held.oneshot(
            Request::post("/v1/utils/models3d/convert?from=glb&to=obj")
                .header("content-type", "application/octet-stream")
                .body(stalled_upload())
                .unwrap(),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let resp = generate(&app).await;
    assert_ne!(resp.status(), StatusCode::SERVICE_UNAVAILABLE, "a stalled conversion upload held the only generation slot");
    assert!(resp.status().is_success(), "{}", resp.status());
    upload.abort();
}

/// 2026-10-31 audit: every body-read failure came back as 413
/// `body_too_large`; one that breaks off is the client's broken request.
#[tokio::test]
async fn an_upload_that_breaks_off_is_400_not_413() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let broken = || {
        Body::from_stream(futures::stream::iter(vec![
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"glTF")),
            Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset")),
        ]))
    };
    let resp = post_convert(&app, "?from=glb&to=obj", broken).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(harness::json_body(resp).await["error"]["code"], "body_unreadable");
}

/// 2026-10-31 audit: a repeated parameter got axum's plain-text rejection
/// (exactly what the `Option` query fields exist to avoid), and an unknown
/// one — an option the converter does not have — was silently ignored.
#[tokio::test]
async fn repeated_and_unknown_parameters_get_litegen_errors() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (query, code, param) in [
        ("?from=glb&from=obj&to=stl", "param_repeated", "from"),
        ("?from=glb&to=stl&self_contained=false", "param_unsupported", "self_contained"),
    ] {
        let resp = convert(&app, query, cube_glb()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = harness::json_body(resp).await;
        assert_eq!(body["error"]["code"], code, "{query}: {body}");
        assert_eq!(body["error"]["param"], param, "{query}: {body}");
    }
}

/// 2026-10-31 audit: generation delivery re-encodes WebP textures as PNG
/// (so no container drops them and no glTF requires `EXT_texture_webp`); the
/// converter did not, so a WebP-textured GLB lost its texture in USDZ, FBX and
/// 3MF and came back as a GLB requiring the extension.
#[tokio::test]
async fn webp_textures_survive_conversion_as_png() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let mut scene = mesh::fixtures::textured();
    let img = image::load_from_memory_with_format(&scene.images[0].data, image::ImageFormat::Png).unwrap();
    let mut webp = Vec::new();
    image::codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .encode(img.as_bytes(), img.width(), img.height(), img.color().into())
        .unwrap();
    scene.images[0].format = mesh::scene::ImageFormat::Webp;
    scene.images[0].data = webp;
    let glb = mesh::glb::write_scene(&scene).unwrap();
    for to in ["usdz", "fbx", "3mf"] {
        let resp = convert(&app, &format!("?from=glb&to={to}"), glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{to}");
        let losses = resp.headers().get("x-litegen-conversion-losses").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        assert!(!losses.contains("image_formats") && !losses.contains("textures"), "{to}: {losses}");
        let back = mesh::read_scene(&body_bytes(resp).await, MeshFormat::parse(to).unwrap(), &mesh::NoSidecars).unwrap().scene;
        assert!(back.materials.iter().any(|m| m.base_color_texture.is_some()), "{to}: the texture is kept");
    }
    let resp = convert(&app, "?from=glb&to=glb", glb).await;
    let out = body_bytes(resp).await;
    let json_len = u32::from_le_bytes(out[12..16].try_into().unwrap()) as usize;
    let json = std::str::from_utf8(&out[20..20 + json_len]).unwrap();
    assert!(!json.contains("EXT_texture_webp"), "{json}");
}

/// 2026-10-12 review, finding 29: the losses header was built from what the
/// writer dropped only; what the READER dropped — a material library the
/// upload could not include, an extension litegen does not carry — vanished.
/// It is now named in `X-Litegen-Conversion-Warnings`.
#[tokio::test]
async fn what_the_reader_dropped_is_named_in_the_warnings_header() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let obj = b"mtllib meshy.mtl\nusemtl m\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n".to_vec();
    let resp = convert(&app, "?from=obj&to=glb", obj).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let warnings = resp.headers().get("x-litegen-conversion-warnings").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
    assert!(warnings.contains("meshy.mtl"), "{warnings:?}");
    // A clean file gets no header.
    let resp = convert(&app, "?from=glb&to=obj", cube_glb()).await;
    assert!(resp.headers().get("x-litegen-conversion-warnings").is_none());
}

fn bounds_of(bytes: &[u8], format: MeshFormat) -> ([f32; 3], [f32; 3]) {
    mesh::read(bytes, format).unwrap().bounds().unwrap()
}

async fn error_code(resp: axum::response::Response) -> (StatusCode, String) {
    let status = resp.status();
    let v: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    (status, v["error"]["code"].as_str().unwrap_or_default().to_string())
}

/// 2026-10-12 review, finding 24.12: STL, OBJ and PLY carry no units or up
/// axis, and are written in glTF's frame, metres with +Y up. `units` and `up`
/// say what the file should be in instead (a slicer reads millimetres, +Z
/// up), and `from_units`/`from_up` how to read one; turning +Y up into +Z up
/// is the turn about +X that sends glTF's front (+Z) to −Y, as Blender's
/// glTF importer does. A container with units and an axis of its own
/// refuses them, and an unknown value is a 400.
#[tokio::test]
async fn units_and_up_axis_apply_to_containers_that_have_none() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = cube_glb();
    let (lo, hi) = bounds_of(&mesh::convert(&glb, MeshFormat::Glb, MeshFormat::Stl).unwrap(), MeshFormat::Stl);
    for to in ["stl", "obj", "ply"] {
        let fmt = MeshFormat::parse(to).unwrap();
        let resp = convert(&app, &format!("?from=glb&to={to}&units=mm&up=z"), glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{to}");
        let out = body_bytes(resp).await;
        let (glo, ghi) = bounds_of(&out, fmt);
        let want_lo = [lo[0] * 1000.0, -hi[2] * 1000.0, lo[1] * 1000.0];
        let want_hi = [hi[0] * 1000.0, -lo[2] * 1000.0, hi[1] * 1000.0];
        for k in 0..3 {
            assert!((glo[k] - want_lo[k]).abs() < 1e-2 && (ghi[k] - want_hi[k]).abs() < 1e-2, "{to}: {glo:?}..{ghi:?} vs {want_lo:?}..{want_hi:?}");
        }
        // Read back the way it was written: the original frame.
        let resp = convert(&app, &format!("?from={to}&from_units=mm&from_up=z&to=glb"), out).await;
        assert_eq!(resp.status(), StatusCode::OK, "{to} back");
        let (blo, bhi) = bounds_of(&body_bytes(resp).await, MeshFormat::Glb);
        for k in 0..3 {
            assert!((blo[k] - lo[k]).abs() < 1e-5 && (bhi[k] - hi[k]).abs() < 1e-5, "{to} back: {blo:?}..{bhi:?} vs {lo:?}..{hi:?}");
        }
    }
    for (query, status, code) in [
        ("?from=glb&to=glb&units=mm", StatusCode::BAD_REQUEST, "param_unsupported"),
        ("?from=glb&to=fbx&up=z", StatusCode::BAD_REQUEST, "param_unsupported"),
        ("?from=glb&to=stl&from_units=mm", StatusCode::BAD_REQUEST, "param_unsupported"),
        ("?from=glb&to=stl&units=furlong", StatusCode::BAD_REQUEST, "param_invalid"),
        ("?from=glb&to=stl&up=x", StatusCode::BAD_REQUEST, "param_invalid"),
    ] {
        assert_eq!(error_code(convert(&app, query, glb.clone()).await).await, (status, code.to_string()), "{query}");
    }
}

/// A Blender-style FBX: one triangle, a Phong material with DiffuseColor 0.8.
fn classic_fbx() -> Vec<u8> {
    "; FBX 7.4.0 project file\nFBXHeaderExtension:  {\n\tFBXVersion: 7400\n\tCreator: \"Blender (stable FBX IO) - 4.4.0\"\n}\nObjects:  {\n\
     \tModel: 10, \"Model::m\", \"Mesh\" {\n\t}\n\
     \tGeometry: 99, \"Geometry::tri\", \"Mesh\" {\n\t\tVertices: *9 {\n\t\t\ta: 0,0,0,1,0,0,0,1,0\n\t\t}\n\t\tPolygonVertexIndex: *3 {\n\t\t\ta: 0,1,-3\n\t\t}\n\
     \t\tLayerElementMaterial: 0 {\n\t\t\tMappingInformationType: \"AllSame\"\n\t\t\tReferenceInformationType: \"IndexToDirect\"\n\t\t\tMaterials: *1 {\n\t\t\t\ta: 0\n\t\t\t}\n\t\t}\n\t}\n\
     \tMaterial: 20, \"Material::m\", \"\" {\n\t\tShadingModel: \"phong\"\n\t\tProperties70:  {\n\t\t\tP: \"DiffuseColor\", \"Color\", \"\", \"A\",0.8,0.8,0.8\n\t\t}\n\t}\n\
     }\nConnections:  {\n\tC: \"OO\",10,0\n\tC: \"OO\",99,10\n\tC: \"OO\",20,10\n}\n"
        .as_bytes()
        .to_vec()
}

/// Review finding 21, left to the client: `fbx_color_space` says how an FBX's
/// material colours are stored, at whichever end is FBX. A Blender-style
/// DiffuseColor 0.8 is 0.8 when the caller says `linear`, and 0.604 (decoded
/// as sRGB) by default, unchanged. It is refused where no end is FBX, and a
/// value it does not take is a 400.
#[tokio::test]
async fn the_fbx_colour_space_is_the_caller_s_choice() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let base = |bytes: Vec<u8>| mesh::read_scene(&bytes, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene.materials[0].base_color[0];
    let linear = convert(&app, "?from=fbx&to=glb&fbx_color_space=linear", classic_fbx()).await;
    assert_eq!(linear.status(), StatusCode::OK);
    assert!((base(body_bytes(linear).await) - 0.8).abs() < 1e-4);
    let default = convert(&app, "?from=fbx&to=glb", classic_fbx()).await;
    assert!((base(body_bytes(default).await) - mesh::math::srgb_to_linear(0.8)).abs() < 1e-4);
    let srgb = convert(&app, "?from=glb&to=fbx&fbx_color_space=srgb", cube_glb()).await;
    assert_eq!(srgb.status(), StatusCode::OK);
    drop(srgb); // an unread file holds its conversion slot
    for (query, status, code) in [
        ("?from=glb&to=obj&fbx_color_space=linear", StatusCode::BAD_REQUEST, "param_unsupported"),
        ("?from=fbx&to=glb&fbx_color_space=gamma", StatusCode::BAD_REQUEST, "param_invalid"),
    ] {
        assert_eq!(error_code(convert(&app, query, classic_fbx()).await).await, (status, code.to_string()), "{query}");
    }
}

// ── `fbx_light_units`, `usd_light_units`: the units a file's lights are stored in ──

/// `fixtures::staged()` (a sun, a bulb and a spot) written as `format` with `opts`.
fn staged_as(format: MeshFormat, opts: mesh::WriteOptions) -> Vec<u8> {
    mesh::write_scene(&mesh::fixtures::staged(), format, opts).unwrap().package.files.remove(0).bytes
}

/// Each light's name and intensity, as `bytes` (in `format`) read with `opts` says.
fn lights_of(bytes: &[u8], format: MeshFormat, opts: mesh::ReadOptions) -> Vec<(String, f64)> {
    let read = mesh::read_scene_with(bytes, format, &mesh::NoSidecars, opts).unwrap();
    read.scene.lights.iter().map(|l| (l.name.clone().unwrap_or_default(), f64::from(l.intensity))).collect()
}

/// `got` against `want`, within `rel` of `want`.
fn near(got: f64, want: f64, rel: f64, ctx: &str) {
    assert!((got - want).abs() <= rel * want.abs(), "{ctx}: {got} against {want}");
}

/// `fbx_light_units` and `usd_light_units` say what units a file's light
/// strengths are stored in, `photometric` (the default: FBX `Intensity` is
/// 100 × the candela or lux, USD's are the normalised values) or `blender`
/// (the `energy` Blender's glTF import gives the light), each at whichever end
/// of the conversion is its format. They are refused where no end is that
/// format, and a word they do not take is a 400.
#[tokio::test]
async fn light_units_apply_at_their_end() {
    use mesh::scene::blender_energy;
    use mesh::LightUnits::Blender;
    let (app, _db) = harness::app_with_mock_3d().await;
    let staged = mesh::fixtures::staged();
    let staged_glb = staged_as(MeshFormat::Glb, mesh::WriteOptions::default());

    // Written to FBX, `Intensity` is 100 × E. A photometric read divides the
    // 100 out and leaves E; a Blender read turns E back into the scene's own.
    let resp = convert(&app, "?from=glb&to=fbx&fbx_light_units=blender", staged_glb.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let fbx = body_bytes(resp).await;
    let as_energy = lights_of(&fbx, MeshFormat::Fbx, mesh::ReadOptions::default());
    let in_blender = lights_of(&fbx, MeshFormat::Fbx, mesh::ReadOptions { fbx_light_units: Blender, ..Default::default() });
    assert_eq!(as_energy.len(), 3, "{as_energy:?}");
    for light in &staged.lights {
        let name = light.name.clone().unwrap();
        let find = |lights: &[(String, f64)]| lights.iter().find(|l| l.0 == name).unwrap_or_else(|| panic!("no light {name}: {lights:?}")).1;
        near(find(&as_energy), blender_energy(&light.kind, f64::from(light.intensity)), 1e-6, &format!("{name}: Intensity / 100 is E"));
        near(find(&in_blender), f64::from(light.intensity), 1e-6, &format!("{name}: E turned back"));
    }
    // Asking for photometric is the default, byte for byte, and Blender's units
    // change the lights alone: the file is not the same, and it is as long.
    let default = body_bytes(convert(&app, "?from=glb&to=fbx", staged_glb.clone()).await).await;
    let photometric = body_bytes(convert(&app, "?from=glb&to=fbx&fbx_light_units=photometric", staged_glb.clone()).await).await;
    assert!(photometric == default, "photometric is today's bytes");
    assert!(fbx != default && fbx.len() == default.len(), "blender changes the lights' numbers and nothing else");

    // Read from FBX: the same Blender-unit file comes back as the scene's own
    // strengths with the option, and as E without it.
    let blender_fbx = staged_as(MeshFormat::Fbx, mesh::WriteOptions { fbx_light_units: Blender, ..Default::default() });
    let resp = convert(&app, "?from=fbx&to=glb&fbx_light_units=blender", blender_fbx.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let glb = body_bytes(resp).await;
    for (name, got) in lights_of(&glb, MeshFormat::Glb, mesh::ReadOptions::default()) {
        let want = staged.lights.iter().find(|l| l.name.as_deref() == Some(name.as_str())).unwrap();
        near(got, f64::from(want.intensity), 1e-6, &format!("{name}: read in Blender's units"));
    }
    let glb = body_bytes(convert(&app, "?from=fbx&to=glb", blender_fbx).await).await;
    let sun = lights_of(&glb, MeshFormat::Glb, mesh::ReadOptions::default()).into_iter().find(|l| l.0 == "sun").unwrap().1;
    near(sun, blender_energy(&mesh::scene::LightKind::Directional, 3.0), 1e-6, "read as photometric, the sun is its energy");

    // USD: the option writes Blender's UsdLux values, and the file's own
    // `customData` still gives litegen back the scene's strengths.
    let resp = convert(&app, "?from=glb&to=usdz&usd_light_units=blender", staged_glb.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let usdz = body_bytes(resp).await;
    let default_usdz = body_bytes(convert(&app, "?from=glb&to=usdz", staged_glb.clone()).await).await;
    assert!(usdz != default_usdz, "blender writes other UsdLux values");
    for (name, got) in lights_of(&usdz, MeshFormat::Usdz, mesh::ReadOptions::default()) {
        let want = staged.lights.iter().find(|l| l.name.as_deref() == Some(name.as_str())).unwrap();
        near(got, f64::from(want.intensity), 1e-6, &format!("{name}: usdz customData"));
    }
    let usda = body_bytes(convert(&app, "?from=glb&to=usda&usd_light_units=blender", staged_glb.clone()).await).await;
    assert!(usda != body_bytes(convert(&app, "?from=glb&to=usda", staged_glb.clone()).await).await);
    // A crate takes the option as a text layer does (Ruling U11), at either end.
    let usdc = body_bytes(convert(&app, "?from=glb&to=usdc&usd_light_units=blender", staged_glb.clone()).await).await;
    assert!(usdc != body_bytes(convert(&app, "?from=glb&to=usdc", staged_glb.clone()).await).await, "blender writes other UsdLux values in a crate too");
    for (name, got) in lights_of(&usdc, MeshFormat::Usdc, mesh::ReadOptions::default()) {
        let want = staged.lights.iter().find(|l| l.name.as_deref() == Some(name.as_str())).unwrap();
        near(got, f64::from(want.intensity), 1e-6, &format!("{name}: usdc customData"));
    }
    let resp = convert(&app, "?from=usdc&to=glb&usd_light_units=blender", usdc).await;
    assert_eq!(resp.status(), StatusCode::OK, "usd_light_units applies where `from` is usdc");

    // Spaces and case do not matter.
    let resp = convert(&app, "?from=glb&to=fbx&fbx_light_units=%20Blender%20", staged_glb.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(body_bytes(resp).await == fbx, "%20Blender%20 is blender");

    // The same format at both ends reads Blender's units and writes them back:
    // a litegen-written FBX keeps its light strengths (the no-op the option
    // documents; to retarget, convert twice).
    let photometric_fbx = staged_as(MeshFormat::Fbx, mesh::WriteOptions::default());
    let was = lights_of(&photometric_fbx, MeshFormat::Fbx, mesh::ReadOptions::default());
    let resp = convert(&app, "?from=fbx&to=fbx&fbx_light_units=blender", photometric_fbx).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let is = lights_of(&body_bytes(resp).await, MeshFormat::Fbx, mesh::ReadOptions::default());
    for ((name, was), (_, is)) in was.iter().zip(&is) {
        near(*is, *was, 1e-6, &format!("{name}: a litegen staged FBX through fbx to fbx with blender"));
    }

    // The USD read end (the end `from` names): a USDA without litegen's
    // `customData`, as Blender writes one, reads by Blender's importer's
    // formula with the option and by the raw UsdLux values without it. Its
    // sun's 0.0010980966 and bulb's 0.11713031 are 3 lux and 20 cd to
    // Blender's importer (`usd_blender_read_follows_blenders_importer`).
    let blender_usda = "#usda 1.0\n(\n    defaultPrim = \"R\"\n    metersPerUnit = 1\n    upAxis = \"Y\"\n)\ndef Xform \"R\"\n{\n\
        def Mesh \"tri\" {\n int[] faceVertexCounts = [3]\n int[] faceVertexIndices = [0, 1, 2]\n point3f[] points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]\n}\n\
        def DistantLight \"sun\" {\n float inputs:intensity = 0.0010980966\n bool inputs:normalize = 1\n float inputs:angle = 0\n}\n\
        def SphereLight \"bulb\" {\n float inputs:intensity = 0.11713031\n bool inputs:normalize = 1\n float inputs:radius = 0\n bool treatAsPoint = 1\n}\n}\n";
    let light = |lights: &[(String, f64)], name: &str| lights.iter().find(|l| l.0 == name).unwrap_or_else(|| panic!("no light {name}: {lights:?}")).1;
    let resp = convert(&app, "?from=usda&to=glb&usd_light_units=blender", blender_usda.as_bytes().to_vec()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let read = lights_of(&body_bytes(resp).await, MeshFormat::Glb, mesh::ReadOptions::default());
    near(light(&read, "sun"), 3.0, 1e-4, "usda to glb with blender: the sun is Blender's importer's");
    near(light(&read, "bulb"), 20.0, 1e-4, "usda to glb with blender: the bulb is Blender's importer's");
    let resp = convert(&app, "?from=usda&to=glb", blender_usda.as_bytes().to_vec()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let raw = lights_of(&body_bytes(resp).await, MeshFormat::Glb, mesh::ReadOptions::default());
    near(light(&raw, "sun"), 0.0010980966, 1e-5, "usda to glb without the option: the raw UsdLux value");
    near(light(&raw, "bulb"), 0.11713031 / 4.0, 1e-5, "usda to glb without the option: a normalised sphere's cd is its intensity / 4");
    // A usdz is a read end as well, and the usd option is not the fbx one.
    let resp = convert(&app, "?from=usdz&to=usda&usd_light_units=blender", staged_as(MeshFormat::Usdz, mesh::WriteOptions::default())).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let usda_back = lights_of(&body_bytes(resp).await, MeshFormat::Usda, mesh::ReadOptions::default());
    assert_eq!(usda_back.len(), 3, "{usda_back:?}");
    for (name, got) in &usda_back {
        let want = staged.lights.iter().find(|l| l.name.as_deref() == Some(name.as_str())).unwrap();
        near(*got, f64::from(want.intensity), 1e-6, &format!("{name}: usdz to usda with blender keeps the scene's strengths (customData)"));
    }

    // Each refusal and its message, word for word: litegen-d0's converter
    // package says the same.
    let wrong_fbx = "`fbx_light_units` applies when `from` or `to` is fbx";
    let wrong_usd = "`usd_light_units` applies when `from` or `to` is usdz, usda or usdc";
    for (query, code, param, message) in [
        ("?from=glb&to=obj&fbx_light_units=blender", "param_unsupported", "fbx_light_units", Some(wrong_fbx)),
        ("?from=usda&to=glb&fbx_light_units=blender", "param_unsupported", "fbx_light_units", Some(wrong_fbx)),
        ("?from=glb&to=fbx&usd_light_units=blender", "param_unsupported", "usd_light_units", Some(wrong_usd)),
        ("?from=glb&to=obj&usd_light_units=blender", "param_unsupported", "usd_light_units", Some(wrong_usd)),
        ("?from=glb&to=fbx&fbx_light_units=watts", "param_invalid", "fbx_light_units", Some("`fbx_light_units` is \"watts\"; it takes photometric or blender")),
        ("?from=glb&to=usdz&usd_light_units=watts", "param_invalid", "usd_light_units", Some("`usd_light_units` is \"watts\"; it takes photometric or blender")),
        ("?from=glb&to=fbx&fbx_light_units=%20Watts%20", "param_invalid", "fbx_light_units", Some("`fbx_light_units` is \" Watts \"; it takes photometric or blender")),
        ("?from=glb&to=fbx&fbx_light_units=", "param_invalid", "fbx_light_units", Some("`fbx_light_units` is \"\"; it takes photometric or blender")),
        ("?from=glb&to=fbx&fbx_light_units=blender&fbx_light_units=blender", "param_repeated", "fbx_light_units", None),
    ] {
        let resp = convert(&app, query, staged_glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = harness::json_body(resp).await;
        assert_eq!((&body["error"]["code"], &body["error"]["param"]), (&serde_json::json!(code), &serde_json::json!(param)), "{query}: {body}");
        if let Some(message) = message {
            assert_eq!(body["error"]["message"], message, "{query}: {body}");
        }
    }
}

/// The two light-units parameters are optional strings on the endpoint, each
/// saying what its two words mean, whose reading `photometric` is, and where
/// it applies; the 400 names both.
#[tokio::test]
async fn the_light_units_parameters_are_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let post = &spec["paths"]["/v1/utils/models3d/convert"]["post"];
    let fbx_words: &[&str] = &["fbx", "photometric", "blender", "three.js", "ufbx", "Intensity", "683"];
    // Photometric USD is UsdLux's by the schema; Storm agrees on the sun and
    // draws a sphere 4x brighter (`usd_write.rs`), so it is not "Storm's reading".
    let usd_words: &[&str] = &[
        "usdz",
        "photometric",
        "blender",
        "UsdLux's own, by the schema",
        "(Storm, usdview's renderer, agrees on the sun and draws such a sphere 4× brighter)",
        "customData",
        "683",
    ];
    for (name, words) in [("fbx_light_units", fbx_words), ("usd_light_units", usd_words)] {
        let param = post["parameters"]
            .as_array()
            .and_then(|ps| ps.iter().find(|p| p["name"] == name))
            .unwrap_or_else(|| panic!("no `{name}` parameter: {}", post["parameters"]));
        assert_eq!(param["in"], "query");
        assert_ne!(param["required"], true, "optional: photometric is the default");
        let said = param["description"].as_str().unwrap_or_default();
        for word in words {
            assert!(said.contains(word), "{name}: {word}: {said}");
        }
        // The same-format sentence: FBX has no `customData`, so its pass is a
        // no-op for any file; a USD litegen wrote is rewritten, so USD's says
        // what is true of a file in Blender's units and of one litegen wrote.
        let same_format = if name == "fbx_light_units" {
            "The option applies at whichever end is fbx, so `from=fbx&to=fbx` with `blender` reads Blender's units and writes them back, a no-op: to move a Blender file to `photometric`, convert twice (fbx to glb with `blender`, then glb to fbx)."
        } else {
            "The option applies at whichever end is usdz, usda or usdc: `from=usdz&to=usdz` with `blender` keeps the strengths of a file in Blender's units, and rewrites those of a file litegen wrote in Blender's units, its `customData` keeping the exact values. To move a Blender file to `photometric`, convert twice (usdz to glb with `blender`, then glb to usdz)."
        };
        assert!(said.ends_with(same_format), "{name}: the same-format sentence: {said}");
        let refusals = post["responses"]["400"]["description"].as_str().unwrap_or_default();
        assert!(refusals.contains(name), "{name}: {refusals}");
    }
}

// ── `package=zip`: the file and its sidecars, as one zip ──

/// The `textured` fixture as a GLB: three materials, one with a PNG map.
fn textured_glb() -> Vec<u8> {
    mesh::write_scene(&mesh::fixtures::textured(), MeshFormat::Glb, mesh::WriteOptions::default())
        .unwrap()
        .package
        .files
        .remove(0)
        .bytes
}

/// A zip body's entries, in order, read by litegen's own archive reader.
fn unzip(bytes: &[u8]) -> Vec<mesh::PackageFile> {
    let a = mesh::zip::Archive::parse(bytes).expect("the body is a zip");
    a.entries.iter().map(|e| mesh::PackageFile { path: e.name.clone(), bytes: a.read(e).unwrap() }).collect()
}

fn loss_kinds(resp: &axum::response::Response) -> Vec<String> {
    resp.headers()
        .get("x-litegen-conversion-losses")
        .map(|v| v.to_str().unwrap().split(',').map(str::to_string).collect())
        .unwrap_or_default()
}

/// The headers a converted file is described by.
fn file_headers(resp: &axum::response::Response) -> Vec<(&'static str, Option<String>)> {
    ["content-type", "content-disposition", "content-length", "x-litegen-conversion-losses", "x-litegen-conversion-warnings"]
        .into_iter()
        .map(|h| (h, resp.headers().get(h).map(|v| v.to_str().unwrap().to_string())))
        .collect()
}

#[tokio::test]
async fn package_zip_returns_the_whole_package() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = textured_glb();
    let zipped = convert(&app, "?from=glb&to=obj&package=zip", glb.clone()).await;
    assert_eq!(zipped.status(), StatusCode::OK);
    assert_eq!(zipped.headers()["content-type"], "application/zip");
    assert_eq!(zipped.headers()["content-disposition"], "attachment; filename=\"model.obj.zip\"");
    let package_losses = loss_kinds(&zipped);
    let files = unzip(&body_bytes(zipped).await);
    let names: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(names, ["model.obj", "model.mtl", "textures/quadrants.png"]);
    let back = mesh::read_scene(&files[0].bytes, MeshFormat::Obj, &mesh::package::MemorySidecars { files: &files }).unwrap().scene;
    assert_eq!(back.materials.len(), 3, "the MTL's materials");
    assert!(back.materials.iter().any(|m| m.base_color_texture.is_some()), "the map, read from the zip's own texture file");
    // Judged against the package: exactly what the library reports writing one.
    let scene = mesh::read_scene(&glb, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
    let package_mode = mesh::write_scene(&scene, MeshFormat::Obj, mesh::WriteOptions { self_contained: false, ..Default::default() }).unwrap();
    assert_eq!(package_losses.join(","), expected_losses_header(&package_mode.losses).unwrap_or_default());
    // What one file loses, the package keeps: the texture here, and the
    // materials of a scene whose materials MTL can state whole (the panel's
    // double-sidedness it cannot, in either form, so it is set aside).
    let single_losses = loss_kinds(&convert(&app, "?from=glb&to=obj", glb).await);
    assert!(single_losses.iter().any(|k| k == "textures"), "one OBJ file loses its texture: {single_losses:?}");
    assert!(!package_losses.iter().any(|k| k == "textures"), "the package keeps its texture: {package_losses:?}");
    let mut one_sided = mesh::fixtures::textured();
    for m in &mut one_sided.materials {
        m.double_sided = false;
    }
    let one_sided = mesh::glb::write_scene(&one_sided).unwrap();
    let single = loss_kinds(&convert(&app, "?from=glb&to=obj", one_sided.clone()).await);
    let zipped = loss_kinds(&convert(&app, "?from=glb&to=obj&package=zip", one_sided).await);
    assert!(single.iter().any(|k| k == "materials"), "one OBJ file bakes its materials into vertex colours: {single:?}");
    assert!(!zipped.iter().any(|k| k == "materials"), "the package's MTL keeps them: {zipped:?}");
}

#[tokio::test]
async fn package_file_is_todays_response() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = textured_glb();
    for to in derivable_formats() {
        let today = convert(&app, &format!("?from=glb&to={to}"), glb.clone()).await;
        let file = convert(&app, &format!("?from=glb&to={to}&package=file"), glb.clone()).await;
        assert_eq!(today.status(), StatusCode::OK, "{to}");
        assert_eq!(file.status(), StatusCode::OK, "{to}");
        assert_eq!(file_headers(&file), file_headers(&today), "{to}");
        assert_eq!(body_bytes(file).await, body_bytes(today).await, "{to}");
    }
}

#[tokio::test]
async fn a_single_file_container_zips_to_one_entry() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = textured_glb();
    for to in ["glb", "stl", "fbx", "usdz", "3mf", "ply"] {
        let zipped = convert(&app, &format!("?from=glb&to={to}&package=zip"), glb.clone()).await;
        assert_eq!(zipped.status(), StatusCode::OK, "{to}");
        assert_eq!(zipped.headers()["content-type"], "application/zip", "{to}");
        assert_eq!(zipped.headers()["content-disposition"], format!("attachment; filename=\"model.{to}.zip\"").as_str(), "{to}");
        let files = unzip(&body_bytes(zipped).await);
        let file = body_bytes(convert(&app, &format!("?from=glb&to={to}"), glb.clone()).await).await;
        assert_eq!(files.len(), 1, "{to}");
        assert_eq!(files[0].path, format!("model.{to}"), "{to}");
        assert!(files[0].bytes == file, "{to}: the one entry is the file response, byte for byte");
    }
}

#[tokio::test]
async fn package_values_are_checked() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (query, code) in [
        ("?from=glb&to=obj&package=tar", "param_invalid"),
        ("?from=glb&to=obj&package=zip&package=zip", "param_repeated"),
        ("?from=glb&to=obj&package=", "param_invalid"),
    ] {
        let resp = convert(&app, query, cube_glb()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = harness::json_body(resp).await;
        assert_eq!(body["error"]["code"], code, "{query}: {body}");
        assert_eq!(body["error"]["param"], "package", "{query}: {body}");
    }
    for value in ["ZIP", " zip", "File"] {
        let resp = convert(&app, &format!("?from=glb&to=obj&package={}", value.replace(' ', "%20")), cube_glb()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{value:?}: values are case- and space-insensitive, as fbx_color_space's");
        let zip = resp.headers()["content-type"] == "application/zip";
        assert_eq!(zip, value.trim().eq_ignore_ascii_case("zip"), "{value:?}");
        drop(resp); // an unread file holds its conversion slot
    }
}

#[tokio::test]
async fn the_package_parameter_and_the_zip_response_are_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let post = &spec["paths"]["/v1/utils/models3d/convert"]["post"];
    let param = post["parameters"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["name"] == "package"))
        .unwrap_or_else(|| panic!("no `package` parameter: {}", post["parameters"]));
    assert_eq!(param["in"], "query");
    assert_ne!(param["required"], true, "optional: one file is the default");
    let enum_of = |s: &serde_json::Value| -> Option<serde_json::Value> {
        s.get("enum").cloned().or_else(|| s.get("oneOf")?.as_array()?.iter().find_map(|v| v.get("enum").cloned()))
    };
    assert_eq!(enum_of(&param["schema"]), Some(serde_json::json!(["file", "zip"])), "{param}");
    let zip = &post["responses"]["200"]["content"]["application/zip"]["schema"];
    assert_eq!(zip["type"], "string", "{zip}");
    assert_eq!(zip["format"], "binary", "{zip}");
    assert!(post["responses"]["200"]["content"]["application/octet-stream"].is_object(), "the one-file response stays");
}

// ─── KTX2 (Basis Universal) textures (mesh-gaps item 3) ─────────────────────

/// A GLB requiring `KHR_texture_basisu`: the `textured` fixture whose base
/// colour is CarConcept's `Dot_C`, an ETC1S KTX2 (the vendor's file of
/// `mock/basisu-3d`; `mock_data/README.md`).
fn basisu_glb() -> Vec<u8> {
    include_bytes!("../src/providers/model3d/mock_data/textured.basisu.glb").to_vec()
}

/// The KTX2 bytes in `glb`.
fn ktx2_of(glb: &[u8]) -> Vec<u8> {
    let scene = mesh::read_scene(glb, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
    assert_eq!(scene.images[0].format, mesh::scene::ImageFormat::Ktx2);
    scene.images[0].data.clone()
}

/// The first base-colour map of `scene`, as `(image format, image bytes)`.
fn base_colour(scene: &mesh::Scene) -> (mesh::scene::ImageFormat, Vec<u8>) {
    let map = scene.materials.iter().find_map(|m| m.base_color_texture.as_ref()).expect("a base-colour map");
    let image = &scene.images[scene.textures[map.texture].image];
    (image.format, image.data.clone())
}

fn glb_json(glb: &[u8]) -> serde_json::Value {
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&glb[20..20 + len]).unwrap()
}

/// `webp_textures_survive_conversion_as_png` for KTX2: ETC1S here, so no
/// container drops the texture, no loss is reported, and the pixels are the
/// decoder's (`mesh::ktx2`'s own tests prove them the reference transcoder's).
#[tokio::test]
async fn ktx2_textures_survive_conversion_as_png() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = basisu_glb();
    let want = mesh::ktx2::decode_rgba8(&ktx2_of(&glb)).unwrap().pixels;
    for to in ["glb", "gltf", "usdz", "fbx", "3mf"] {
        let resp = convert(&app, &format!("?from=glb&to={to}"), glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{to}");
        let losses = resp.headers().get("x-litegen-conversion-losses").map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        assert!(!losses.contains("image_formats") && !losses.contains("textures"), "{to}: {losses}");
        let out = body_bytes(resp).await;
        let back = mesh::read_scene(&out, MeshFormat::parse(to).unwrap(), &mesh::NoSidecars).unwrap().scene;
        let (format, data) = base_colour(&back);
        assert_eq!(format, mesh::scene::ImageFormat::Png, "{to}");
        assert_eq!(image::load_from_memory(&data).unwrap().to_rgba8().into_raw(), want, "{to}: the decoder's pixels");
        if to == "glb" {
            let doc = glb_json(&out);
            assert!(doc.get("extensionsRequired").is_none() && !doc.to_string().contains("KHR_texture_basisu"), "{doc}");
        }
    }
}

/// A KTX2 litegen cannot decode is still left out and reported, as a broken
/// WebP is: its map goes, and `image_formats` is named.
#[tokio::test]
async fn a_ktx2_that_does_not_decode_keeps_the_image_formats_loss() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let mut scene = mesh::read_scene(&basisu_glb(), MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
    let cut = scene.images[0].data.len() - 200;
    scene.images[0].data.truncate(cut);
    let broken = mesh::glb::write_scene(&scene).unwrap();
    for to in ["fbx", "usdz", "glb"] {
        let resp = convert(&app, &format!("?from=glb&to={to}"), broken.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{to}");
        let kinds = loss_kinds(&resp);
        assert!(kinds.iter().any(|k| k == "image_formats"), "{to}: {kinds:?}");
        let back = mesh::read_scene(&body_bytes(resp).await, MeshFormat::parse(to).unwrap(), &mesh::NoSidecars).unwrap().scene;
        assert!(back.materials.iter().all(|m| m.base_color_texture.is_none()), "{to}: the texture is left out");
    }
}

/// `ktx2_textures=keep` to a glb or gltf: the vendor's KTX2 bytes are the
/// output's image, and the file requires `KHR_texture_basisu`, nothing
/// reported lost.
#[tokio::test]
async fn ktx2_textures_keep_leaves_glb_its_ktx2() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = basisu_glb();
    let ktx2 = ktx2_of(&glb);
    for to in ["glb", "gltf"] {
        let resp = convert(&app, &format!("?from=glb&to={to}&ktx2_textures=keep"), glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{to}");
        assert!(loss_kinds(&resp).is_empty(), "{to}: {:?}", loss_kinds(&resp));
        let out = body_bytes(resp).await;
        let doc: serde_json::Value = if to == "glb" { glb_json(&out) } else { serde_json::from_slice(&out).unwrap() };
        assert_eq!(doc["extensionsRequired"], serde_json::json!(["KHR_texture_basisu"]), "{to}");
        let back = mesh::read_scene(&out, MeshFormat::parse(to).unwrap(), &mesh::NoSidecars).unwrap().scene;
        assert_eq!(base_colour(&back), (mesh::scene::ImageFormat::Ktx2, ktx2.clone()), "{to}: the KTX2, byte for byte");
    }
}

/// `keep` with `package=zip`: a gltf package carries the KTX2 as a file of its
/// own (and its `.gltf` requires `KHR_texture_basisu`), where the default
/// carries a PNG; a glb is the one entry, the `keep` file byte for byte.
#[tokio::test]
async fn ktx2_textures_keep_with_package_zip_carries_the_ktx2() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = basisu_glb();
    let ktx2 = ktx2_of(&glb);
    let images = |files: &[mesh::PackageFile]| -> Vec<(String, Vec<u8>)> {
        files.iter().filter(|f| !f.path.ends_with(".gltf") && !f.path.ends_with(".bin")).map(|f| (f.path.clone(), f.bytes.clone())).collect()
    };
    let kept = unzip(&body_bytes(convert(&app, "?from=glb&to=gltf&package=zip&ktx2_textures=keep", glb.clone()).await).await);
    let doc: serde_json::Value = serde_json::from_slice(&kept[0].bytes).expect("the .gltf comes first");
    assert_eq!(doc["extensionsRequired"], serde_json::json!(["KHR_texture_basisu"]), "{doc}");
    let kept_images = images(&kept);
    assert_eq!(kept_images.len(), 1, "{:?}", kept.iter().map(|f| &f.path).collect::<Vec<_>>());
    assert!(kept_images[0].0.ends_with(".ktx2"), "{}", kept_images[0].0);
    assert_eq!(kept_images[0].1, ktx2, "the vendor's KTX2, byte for byte");

    let plain = unzip(&body_bytes(convert(&app, "?from=glb&to=gltf&package=zip", glb.clone()).await).await);
    let plain_images = images(&plain);
    assert_eq!(plain_images.len(), 1);
    assert!(plain_images[0].0.ends_with(".png"), "{}", plain_images[0].0);
    assert!(serde_json::from_slice::<serde_json::Value>(&plain[0].bytes).unwrap().get("extensionsRequired").is_none());

    let zipped = unzip(&body_bytes(convert(&app, "?from=glb&to=glb&package=zip&ktx2_textures=keep", glb.clone()).await).await);
    let file = body_bytes(convert(&app, "?from=glb&to=glb&ktx2_textures=keep", glb.clone()).await).await;
    assert_eq!(zipped.len(), 1);
    assert!(zipped[0].bytes == file, "the one entry is the file response, byte for byte");
}

/// `keep` is about glb and gltf; a request for any other container gets PNG
/// whichever is chosen, exactly the bytes of no parameter at all. The same
/// `keep` still leaves a glb its KTX2, so a `keep` that is ignored fails
/// here too.
#[tokio::test]
async fn ktx2_textures_keep_still_gives_other_containers_png() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = basisu_glb();
    let kept_glb = body_bytes(convert(&app, "?from=glb&to=glb&ktx2_textures=keep", glb.clone()).await).await;
    assert_eq!(glb_json(&kept_glb)["extensionsRequired"], serde_json::json!(["KHR_texture_basisu"]), "keep is not ignored");
    let back = mesh::read_scene(&kept_glb, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
    assert_eq!(base_colour(&back), (mesh::scene::ImageFormat::Ktx2, ktx2_of(&glb)), "the glb keeps the KTX2, byte for byte");
    for to in ["fbx", "usdz", "3mf", "obj"] {
        let plain = body_bytes(convert(&app, &format!("?from=glb&to={to}"), glb.clone()).await).await;
        let kept = body_bytes(convert(&app, &format!("?from=glb&to={to}&ktx2_textures=keep"), glb.clone()).await).await;
        assert!(kept == plain, "{to}: the same bytes as without the parameter");
        if to != "obj" {
            let back = mesh::read_scene(&kept, MeshFormat::parse(to).unwrap(), &mesh::NoSidecars).unwrap().scene;
            assert_eq!(base_colour(&back).0, mesh::scene::ImageFormat::Png, "{to}");
        }
    }
}

/// `png` is the default: asking for it, in any case or with spaces, is the
/// bytes of no parameter, and so is any file with no KTX2 in it.
#[tokio::test]
async fn ktx2_textures_png_is_todays_output() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = basisu_glb();
    for to in ["glb", "fbx", "usdz"] {
        let plain = body_bytes(convert(&app, &format!("?from=glb&to={to}"), glb.clone()).await).await;
        for value in ["png", "PNG", "%20png"] {
            let asked = body_bytes(convert(&app, &format!("?from=glb&to={to}&ktx2_textures={value}"), glb.clone()).await).await;
            assert!(asked == plain, "{to} {value:?}");
        }
    }
    let cube = cube_glb();
    let plain = body_bytes(convert(&app, "?from=glb&to=obj", cube.clone()).await).await;
    for value in ["png", "keep"] {
        let asked = body_bytes(convert(&app, &format!("?from=glb&to=obj&ktx2_textures={value}"), cube.clone()).await).await;
        assert!(asked == plain, "{value}: nothing KTX2 in the file, nothing changes");
    }
}

#[tokio::test]
async fn ktx2_textures_values_are_checked() {
    let (app, _db) = harness::app_with_mock_3d().await;
    for (query, code) in [
        ("?from=glb&to=glb&ktx2_textures=ktx2", "param_invalid"),
        ("?from=glb&to=glb&ktx2_textures=", "param_invalid"),
        ("?from=glb&to=glb&ktx2_textures=keep&ktx2_textures=keep", "param_repeated"),
    ] {
        let resp = convert(&app, query, basisu_glb()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = harness::json_body(resp).await;
        assert_eq!(body["error"]["code"], code, "{query}: {body}");
        assert_eq!(body["error"]["param"], "ktx2_textures", "{query}: {body}");
    }
    for value in ["KEEP", "%20keep", "Png"] {
        let resp = convert(&app, &format!("?from=glb&to=glb&ktx2_textures={value}"), basisu_glb()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{value:?}: values are case- and space-insensitive, as package's");
        drop(resp); // an unread file holds its conversion slot
    }
}

#[tokio::test]
async fn the_ktx2_textures_parameter_is_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let post = &spec["paths"]["/v1/utils/models3d/convert"]["post"];
    let param = post["parameters"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["name"] == "ktx2_textures"))
        .unwrap_or_else(|| panic!("no `ktx2_textures` parameter: {}", post["parameters"]));
    assert_eq!(param["in"], "query");
    assert_ne!(param["required"], true, "optional: png is the default");
    let enum_of = |s: &serde_json::Value| -> Option<serde_json::Value> {
        s.get("enum").cloned().or_else(|| s.get("oneOf")?.as_array()?.iter().find_map(|v| v.get("enum").cloned()))
    };
    assert_eq!(enum_of(&param["schema"]), Some(serde_json::json!(["png", "keep"])), "{param}");
    let said = param["description"].as_str().unwrap_or_default();
    for word in ["KHR_texture_basisu", "64 MiB", "image_formats", "glb", "gltf"] {
        assert!(said.contains(word), "{word}: {said}");
    }
    assert!(post["description"].as_str().unwrap_or_default().contains("KTX2"), "the endpoint's prose names KTX2 beside WebP");
    let invalid = post["responses"]["400"]["description"].as_str().unwrap_or_default();
    assert!(invalid.contains("ktx2_textures"), "{invalid}");
}

// ── `dae_morphs`: a Collada file's morph targets baked in, or kept as controllers ──

/// The `morph` fixture (two targets, a weight animation) as a GLB.
fn morph_glb() -> Vec<u8> {
    mesh::write_scene(&mesh::fixtures::morph(), MeshFormat::Glb, mesh::WriteOptions::default())
        .unwrap()
        .package
        .files
        .remove(0)
        .bytes
}

/// `dae_morphs=keep` writes the targets as `<morph>` controllers (the
/// library's own `Keep` file, byte for byte) and the losses header no longer
/// names `morph_targets`; `bake`, or nothing, is today's file, which applies
/// the targets and reports their animation as lost. The word is case- and
/// space-insensitive.
#[tokio::test]
async fn dae_morphs_keep_writes_controllers() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = morph_glb();
    let staged = mesh::read_scene(&glb, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
    let written = |dae_morphs| {
        let w = mesh::write_scene(&staged, MeshFormat::Dae, mesh::WriteOptions { dae_morphs, ..Default::default() }).unwrap();
        (w.package.files[0].bytes.clone(), expected_losses_header(&w.losses))
    };
    let (kept_file, kept_losses) = written(mesh::DaeMorphs::Keep);
    let (baked_file, baked_losses) = written(mesh::DaeMorphs::Bake);
    assert!(String::from_utf8_lossy(&kept_file).contains("<morph "), "the library's Keep file holds controllers");
    assert!(!String::from_utf8_lossy(&baked_file).contains("<morph"), "the library's Bake file holds none");

    for word in ["keep", "Keep", "%20KEEP%20"] {
        let resp = convert(&app, &format!("?from=glb&to=dae&dae_morphs={word}"), glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{word}");
        assert!(!loss_kinds(&resp).iter().any(|k| k == "morph_targets"), "{word}: {:?}", loss_kinds(&resp));
        let header = resp.headers().get("x-litegen-conversion-losses").map(|v| v.to_str().unwrap().to_string());
        assert_eq!(header, kept_losses, "{word}");
        let body = body_bytes(resp).await;
        assert!(String::from_utf8_lossy(&body).contains("<morph "), "{word}: the body holds morph controllers");
        assert!(body == kept_file, "{word}: the library's Keep file, byte for byte");
    }

    for query in ["?from=glb&to=dae", "?from=glb&to=dae&dae_morphs=bake", "?from=glb&to=dae&dae_morphs=%20Bake%20"] {
        let resp = convert(&app, query, glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{query}");
        assert!(loss_kinds(&resp).iter().any(|k| k == "morph_targets"), "{query}: baking drops the weight animation: {:?}", loss_kinds(&resp));
        let header = resp.headers().get("x-litegen-conversion-losses").map(|v| v.to_str().unwrap().to_string());
        assert_eq!(header, baked_losses, "{query}");
        let body = body_bytes(resp).await;
        assert!(!String::from_utf8_lossy(&body).contains("<morph"), "{query}: no controllers");
        assert!(body == baked_file, "{query}: today's file, byte for byte");
    }
}

/// `dae_morphs` applies when `to` is dae: anything else is
/// `param_unsupported`, a word it does not take `param_invalid`, and a repeat
/// `param_repeated`, each naming the parameter; the two texts are word for
/// word what litegen-d0's converter package says.
#[tokio::test]
async fn dae_morphs_only_for_dae() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = morph_glb();
    let wrong = "`dae_morphs` applies when `to` is dae";
    for (query, code, message) in [
        ("?from=glb&to=obj&dae_morphs=keep", "param_unsupported", Some(wrong)),
        ("?from=glb&to=glb&dae_morphs=bake", "param_unsupported", Some(wrong)),
        ("?from=glb&to=fbx&dae_morphs=keep", "param_unsupported", Some(wrong)),
        ("?from=glb&to=usda&dae_morphs=keep", "param_unsupported", Some(wrong)),
        // A Collada input does not make it apply: only the file written does.
        ("?from=dae&to=glb&dae_morphs=keep", "param_unsupported", Some(wrong)),
        ("?from=glb&to=dae&dae_morphs=yes", "param_invalid", Some("`dae_morphs` is \"yes\"; it takes bake or keep")),
        ("?from=glb&to=dae&dae_morphs=%20Yes%20", "param_invalid", Some("`dae_morphs` is \" Yes \"; it takes bake or keep")),
        ("?from=glb&to=dae&dae_morphs=", "param_invalid", Some("`dae_morphs` is \"\"; it takes bake or keep")),
        ("?from=glb&to=dae&dae_morphs=keep&dae_morphs=keep", "param_repeated", None),
    ] {
        let resp = convert(&app, query, glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = harness::json_body(resp).await;
        assert_eq!((&body["error"]["code"], &body["error"]["param"]), (&serde_json::json!(code), &serde_json::json!("dae_morphs")), "{query}: {body}");
        if let Some(message) = message {
            assert_eq!(body["error"]["message"], message, "{query}: {body}");
        }
    }
}

/// `dae_morphs` is an optional string on the endpoint, saying what its two
/// words do to a Collada file and which readers open the result; the 400
/// names it.
#[tokio::test]
async fn the_dae_morphs_parameter_is_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let post = &spec["paths"]["/v1/utils/models3d/convert"]["post"];
    let param = post["parameters"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["name"] == "dae_morphs"))
        .unwrap_or_else(|| panic!("no `dae_morphs` parameter: {}", post["parameters"]));
    assert_eq!(param["in"], "query");
    assert_ne!(param["required"], true, "optional: bake is the default");
    let said = param["description"].as_str().unwrap_or_default();
    for word in [
        "Only when `to` is `dae`",
        "`bake` (the default)",
        "drops their weight animation, which every Collada reader opens",
        "`keep`",
        "`<morph>` controllers",
        "SceneKit",
        "Xcode",
        "Godot",
        "three.js's ColladaLoader and assimp refuse the whole file",
    ] {
        assert!(said.contains(word), "{word}: {said}");
    }
    let refusals = post["responses"]["400"]["description"].as_str().unwrap_or_default();
    assert!(refusals.contains("dae_morphs"), "{refusals}");
}

// ── `usdc`, `usdz_root` and `usd_clips` (mesh-gaps item 6) ──

/// The scene read from USDA text, brought to the precision a crate holds its
/// transforms at, to be compared with `==` against the same layer's crate
/// reading, `crate_read`. A copy of `transforms_at_f32` in `src/mesh/usd.rs`'s
/// tests, which this file cannot see; see it for why (rulings U16, U18 and
/// U18b): litegen writes its xformOps as `float3`/`quatf`, a crate stores
/// those as f32 and our text reader keeps the decimal by design. Translation
/// and scale are rounded through f32 (exact). Rotation is held to within 2^-24
/// per component of the crate's (half an f32 ulp at 1, the declared precision
/// of `quatf`), asserted, then taken from the crate, because the reader
/// normalises the quaternion after reading, from the decimal on one side and
/// the f32 on the other, so no input rounding can make them equal.
fn transforms_at_f32(mut text: mesh::scene::Scene, crate_read: &mesh::scene::Scene) -> mesh::scene::Scene {
    use mesh::scene::Transform;
    const HALF_F32_ULP_AT_ONE: f64 = 1.0 / 16_777_216.0; // 2^-24
    let f32d = |x: f64| f64::from(x as f32);
    for (i, (node, other)) in text.nodes.iter_mut().zip(&crate_read.nodes).enumerate() {
        if let (Transform::Trs { translation, rotation, scale }, Transform::Trs { rotation: crate_rotation, .. }) = (&mut node.transform, &other.transform) {
            *translation = translation.map(f32d);
            *scale = scale.map(f32d);
            for (c, axis) in ["x", "y", "z", "w"].into_iter().enumerate() {
                let apart = (rotation[c] - crate_rotation[c]).abs();
                assert!(
                    apart <= HALF_F32_ULP_AT_ONE,
                    "node {i} {:?}: rotation {axis} is {} in the text and {} in the crate, {apart:e} apart, more than 2^-24",
                    node.name,
                    rotation[c],
                    crate_rotation[c]
                );
            }
            *rotation = *crate_rotation;
        }
    }
    text
}

/// `usdc` is a converter format at both ends: `to=usdc` answers with the
/// library's own crate, as `application/octet-stream` (IANA registers no media
/// type for one), that reads back to the scene the text layer does; `from=usdc`
/// reads it, and so does `from=usda` (Ruling U10: either takes either
/// encoding); and what is no USD layer is refused naming usdc.
#[tokio::test]
async fn usdc_converts_both_ways_and_the_crate_reads_back() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = textured_glb();
    let resp = convert(&app, "?from=glb&to=usdc", glb.clone()).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "application/octet-stream");
    assert_eq!(resp.headers()["content-disposition"], "attachment; filename=\"model.usdc\"");
    let crate_bytes = body_bytes(resp).await;
    assert!(crate_bytes.starts_with(b"PXR-USDC"), "a crate, not text");
    assert!(crate_bytes == mesh::convert(&glb, MeshFormat::Glb, MeshFormat::Usdc).unwrap(), "the library's own crate, byte for byte");

    let text = body_bytes(convert(&app, "?from=glb&to=usda", glb.clone()).await).await;
    let read = |bytes: &[u8], format| mesh::read_scene(bytes, format, &mesh::NoSidecars).unwrap_or_else(|e| panic!("read as {format}: {e}")).scene;
    let from_crate = read(&crate_bytes, MeshFormat::Usdc);
    assert!(from_crate == transforms_at_f32(read(&text, MeshFormat::Usda), &from_crate), "the crate and the text layer are one scene (transforms at f32)");

    let via_usdc = body_bytes(convert(&app, "?from=usdc&to=glb", crate_bytes.clone()).await).await;
    assert_eq!(mesh::read_scene(&via_usdc, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene.triangle_count(), read(&crate_bytes, MeshFormat::Usdc).triangle_count());
    assert!(body_bytes(convert(&app, "?from=usda&to=glb", crate_bytes.clone()).await).await == via_usdc, "from=usda reads a crate, as it always did");
    // A text layer holds its transforms at the decimal and its crate at f32 (rulings U16, U18), so
    // their glbs are not the same bytes; the name `from` gives the reader changes nothing.
    assert!(
        body_bytes(convert(&app, "?from=usdc&to=glb", text.clone()).await).await == body_bytes(convert(&app, "?from=usda&to=glb", text.clone()).await).await,
        "from=usdc reads a text layer as from=usda does"
    );

    let resp = convert(&app, "?from=usdc&to=glb", glb).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = harness::json_body(resp).await;
    assert_eq!((&body["error"]["code"], &body["error"]["param"]), (&serde_json::json!("format_mismatch"), &serde_json::json!("from")), "{body}");
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.starts_with("not a usdc file") && message.ends_with("check that `from` (usdc) matches the file"), "{message}");
}

/// `usdz_root=usdc` writes the USDZ with a crate root (the library's own file,
/// byte for byte); `usda`, or nothing, is today's file with a text root. The
/// word is case- and space-insensitive.
#[tokio::test]
async fn usdz_root_usdc_writes_a_crate_rooted_usdz() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = textured_glb();
    let staged = mesh::read_scene(&glb, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
    let written = |usdz_root| mesh::write_scene(&staged, MeshFormat::Usdz, mesh::WriteOptions { usdz_root, ..Default::default() }).unwrap().package.files[0].bytes.clone();
    let first_entry = |usdz: &[u8]| mesh::zip::Archive::parse(usdz).expect("a zip").entries[0].name.clone();
    let (crated, texted) = (written(mesh::UsdzRoot::Usdc), written(mesh::UsdzRoot::Usda));
    assert_eq!((first_entry(&crated).as_str(), first_entry(&texted).as_str()), ("model.usdc", "model.usda"));
    for word in ["usdc", "Usdc", "%20USDC%20"] {
        let resp = convert(&app, &format!("?from=glb&to=usdz&usdz_root={word}"), glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{word}");
        assert!(body_bytes(resp).await == crated, "{word}: the library's crate-rooted USDZ, byte for byte");
    }
    for query in ["?from=glb&to=usdz", "?from=glb&to=usdz&usdz_root=usda", "?from=glb&to=usdz&usdz_root=%20Usda%20"] {
        let resp = convert(&app, query, glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK, "{query}");
        assert!(body_bytes(resp).await == texted, "{query}: today's file");
    }
    // Read back, the crate-rooted package is the scene the default one is.
    let (a, b) = (mesh::read_scene(&crated, MeshFormat::Usdz, &mesh::NoSidecars).unwrap().scene, mesh::read_scene(&texted, MeshFormat::Usdz, &mesh::NoSidecars).unwrap().scene);
    assert!(a == transforms_at_f32(b, &a), "the crate-rooted package reads as the default one (transforms at f32)");
    // `usd_clips` is taken where a USD file is written, in either case and
    // with spaces. (How many clips each word keeps is
    // `usd_clips_decides_how_many_clips_a_converted_usd_keeps`.)
    for to in ["usda", "usdc", "usdz"] {
        for word in ["all", "First", "%20ALL%20"] {
            let resp = convert(&app, &format!("?from=glb&to={to}&usd_clips={word}"), glb.clone()).await;
            assert_eq!(resp.status(), StatusCode::OK, "{to} {word}");
        }
    }
}

/// `usd_clips` decides how many clips a converted USD file keeps (mesh-gaps
/// item 6, Ruling U6): from a GLB of three clips, `all` keeps each of them
/// (named and in order), `first` the first, and nothing asked is the
/// container's default, every clip for a USDA and a USDC and
/// `mesh::USDZ_DEFAULT_CLIPS` for a USDZ. The word is case- and
/// space-insensitive, and the file is the library's own, byte for byte.
#[tokio::test]
async fn usd_clips_decides_how_many_clips_a_converted_usd_keeps() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = mesh::write_scene(&mesh::fixtures::clips(), MeshFormat::Glb, mesh::WriteOptions::default()).unwrap().package.files.remove(0).bytes;
    let every = ["walk", "run", "wave"].map(|n| Some(n.to_string())).to_vec();
    let first = vec![Some("walk".to_string())];
    let usdz_default = match mesh::USDZ_DEFAULT_CLIPS {
        mesh::UsdClips::All => &every,
        mesh::UsdClips::First => &first,
    };
    for (to, format) in [("usda", MeshFormat::Usda), ("usdc", MeshFormat::Usdc), ("usdz", MeshFormat::Usdz)] {
        let clips_of = |bytes: &[u8]| -> Vec<Option<String>> {
            mesh::read_scene(bytes, format, &mesh::NoSidecars).unwrap_or_else(|e| panic!("read as {format}: {e}")).scene.animations.iter().map(|a| a.name.clone()).collect()
        };
        let asked = |query: &str| {
            let (app, glb) = (app.clone(), glb.clone());
            let query = format!("?from=glb&to={to}{query}");
            async move {
                let resp = convert(&app, &query, glb).await;
                assert_eq!(resp.status(), StatusCode::OK, "{query}");
                body_bytes(resp).await
            }
        };
        let (default, all, first_only) = (asked("").await, asked("&usd_clips=all").await, asked("&usd_clips=first").await);
        assert_eq!(clips_of(&default), if format == MeshFormat::Usdz { usdz_default.clone() } else { every.clone() }, "{to}: the default");
        assert_eq!(clips_of(&all), every, "{to}: all");
        assert_eq!(clips_of(&first_only), first, "{to}: first");
        for word in ["%20ALL%20", "All"] {
            assert!(asked(&format!("&usd_clips={word}")).await == all, "{to}: {word} is all");
        }
        for word in ["%20FIRST%20", "First"] {
            assert!(asked(&format!("&usd_clips={word}")).await == first_only, "{to}: {word} is first");
        }
        let written = |usd_clips| {
            let opts = mesh::WriteOptions { self_contained: true, usd_clips, ..Default::default() };
            mesh::convert_scene(&glb, MeshFormat::Glb, &mesh::NoSidecars, format, opts).unwrap().package.files[0].bytes.clone()
        };
        assert!(all == written(Some(mesh::UsdClips::All)), "{to}: the library's own file for all, byte for byte");
        assert!(first_only == written(Some(mesh::UsdClips::First)), "{to}: the library's own file for first, byte for byte");
    }
}

/// `usdz_root` applies when `to` is usdz and `usd_clips` when `to` is usda,
/// usdc or usdz (Ruling U5): anything else is `param_unsupported`, a word it
/// does not take `param_invalid`, and a repeat `param_repeated`, each naming
/// the parameter. A USD input does not make either apply: only the file
/// written does.
#[tokio::test]
async fn usdz_root_and_usd_clips_apply_to_their_containers_only() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let glb = textured_glb();
    let wrong_root = "`usdz_root` applies when `to` is usdz";
    let wrong_clips = "`usd_clips` applies when `to` is usda, usdc or usdz";
    for (query, code, param, message) in [
        ("?from=glb&to=usda&usdz_root=usdc", "param_unsupported", "usdz_root", Some(wrong_root)),
        ("?from=glb&to=usdc&usdz_root=usda", "param_unsupported", "usdz_root", Some(wrong_root)),
        ("?from=glb&to=glb&usdz_root=usda", "param_unsupported", "usdz_root", Some(wrong_root)),
        ("?from=usdz&to=glb&usdz_root=usdc", "param_unsupported", "usdz_root", Some(wrong_root)),
        ("?from=usdz&to=obj&usdz_root=nonsense", "param_unsupported", "usdz_root", Some(wrong_root)),
        ("?from=glb&to=usdz&usdz_root=yes", "param_invalid", "usdz_root", Some("`usdz_root` is \"yes\"; it takes usda or usdc")),
        ("?from=glb&to=usdz&usdz_root=%20Yes%20", "param_invalid", "usdz_root", Some("`usdz_root` is \" Yes \"; it takes usda or usdc")),
        ("?from=glb&to=usdz&usdz_root=first", "param_invalid", "usdz_root", Some("`usdz_root` is \"first\"; it takes usda or usdc")),
        ("?from=glb&to=usdz&usdz_root=", "param_invalid", "usdz_root", Some("`usdz_root` is \"\"; it takes usda or usdc")),
        ("?from=glb&to=usdz&usdz_root=usdc&usdz_root=usdc", "param_repeated", "usdz_root", None),
        ("?from=glb&to=obj&usd_clips=all", "param_unsupported", "usd_clips", Some(wrong_clips)),
        ("?from=glb&to=glb&usd_clips=first", "param_unsupported", "usd_clips", Some(wrong_clips)),
        ("?from=glb&to=fbx&usd_clips=all", "param_unsupported", "usd_clips", Some(wrong_clips)),
        ("?from=glb&to=dae&usd_clips=first", "param_unsupported", "usd_clips", Some(wrong_clips)),
        ("?from=usda&to=glb&usd_clips=all", "param_unsupported", "usd_clips", Some(wrong_clips)),
        ("?from=usdc&to=obj&usd_clips=nonsense", "param_unsupported", "usd_clips", Some(wrong_clips)),
        ("?from=glb&to=usda&usd_clips=yes", "param_invalid", "usd_clips", Some("`usd_clips` is \"yes\"; it takes all or first")),
        ("?from=glb&to=usdc&usd_clips=%20Yes%20", "param_invalid", "usd_clips", Some("`usd_clips` is \" Yes \"; it takes all or first")),
        ("?from=glb&to=usdz&usd_clips=usda", "param_invalid", "usd_clips", Some("`usd_clips` is \"usda\"; it takes all or first")),
        ("?from=glb&to=usdz&usd_clips=", "param_invalid", "usd_clips", Some("`usd_clips` is \"\"; it takes all or first")),
        ("?from=glb&to=usda&usd_clips=all&usd_clips=all", "param_repeated", "usd_clips", None),
    ] {
        let resp = convert(&app, query, glb.clone()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{query}");
        let body = harness::json_body(resp).await;
        assert_eq!((&body["error"]["code"], &body["error"]["param"]), (&serde_json::json!(code), &serde_json::json!(param)), "{query}: {body}");
        if let Some(message) = message {
            assert_eq!(body["error"]["message"], message, "{query}: {body}");
        }
    }
}

/// The two parameters are optional strings on the endpoint, each saying what
/// its words do and where it applies; the 400 names both, and `usdc` is a
/// format.
#[tokio::test]
async fn the_usdz_root_and_usd_clips_parameters_are_in_the_openapi_spec() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let resp = app.clone().oneshot(Request::get("/openapi.json").body(Body::empty()).unwrap()).await.unwrap();
    let spec = harness::json_body(resp).await;
    let post = &spec["paths"]["/v1/utils/models3d/convert"]["post"];
    for (name, words) in [
        (
            "usdz_root",
            vec![
                "Only when `to` is `usdz`",
                "`usda` (the default)",
                "`usdchecker --arkit`",
                "`<model-viewer>`",
                "`usdc` is the binary crate, as Apple's own packagers write it",
                "64-byte aligned",
            ],
        ),
        (
            "usd_clips",
            vec![
                "Only when `to` is `usda`, `usdc` or `usdz`",
                "`all` keeps every clip as an alternative",
                "variant set `Animation`",
                "`first` keeps the first clip",
                "`all` for `usda` and `usdc`",
                "AR Quick Look",
            ],
        ),
    ] {
        let param = post["parameters"]
            .as_array()
            .and_then(|ps| ps.iter().find(|p| p["name"] == name))
            .unwrap_or_else(|| panic!("no `{name}` parameter: {}", post["parameters"]));
        assert_eq!(param["in"], "query");
        assert_ne!(param["required"], true, "optional: {name} has a default");
        let said = param["description"].as_str().unwrap_or_default();
        for word in words {
            assert!(said.contains(word), "{name}: {word}: {said}");
        }
        let refusals = post["responses"]["400"]["description"].as_str().unwrap_or_default();
        assert!(refusals.contains(name), "{name}: {refusals}");
    }
    let from = post["parameters"].as_array().and_then(|ps| ps.iter().find(|p| p["name"] == "from")).expect("`from`");
    assert!(from["schema"].to_string().contains("Model3dFormat"), "{from}");
    assert!(from["description"].as_str().unwrap_or_default().contains("`usdc`"), "{from}");
}
