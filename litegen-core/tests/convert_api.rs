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

async fn convert(app: &Router, query: &str, body: Vec<u8>) -> axum::response::Response {
    app.clone()
        .oneshot(
            Request::post(format!("/v1/utils/models3d/convert{query}"))
                .header("content-type", "application/octet-stream")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
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
            let want = mesh::convert_scene(&glb, MeshFormat::Glb, &mesh::NoSidecars, to_fmt, mesh::WriteOptions { self_contained: true })
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
