//! Standalone mesh conversion: [`crate::mesh`] as a utility, with no generation.
//!
//! The generation path already converts. When a vendor returns only GLB,
//! `proxy::model3d_convert` derives the other containers the caller asked for.
//! These endpoints expose the same converter on its own, for a mesh the caller
//! already has: one from an earlier generation, or one that never came from
//! litegen. No provider is called, no generation row is written and nothing is
//! billed.
//!
//! The body is untrusted and goes straight into the mesh parsers, so the
//! handler takes the precautions the generation path does: an in-flight slot
//! before the body is buffered, a hard size cap, and the parse on a blocking
//! thread.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::api::middleware::AppState;
use crate::mesh::{self, MeshError, MeshFormat};
use crate::proxy::model3d_convert::derivable_formats;
use crate::types::{ErrorDetail, ErrorResponse};

/// Query for `POST /v1/utils/models3d/convert`.
///
/// Both are `Option` so a missing one gets litegen's JSON error naming the
/// supported set, rather than axum's plain-text query rejection.
#[derive(Debug, Deserialize)]
pub struct ConvertQuery {
    pub from: Option<String>,
    pub to: Option<String>,
}

/// A mesh file as raw bytes, for the spec only: the handler reads the `Body`
/// directly. Without this, `Vec<u8>` documents as an array of integers and the
/// SDK generators type the upload as `number[]`.
#[derive(utoipa::ToSchema)]
#[schema(value_type = String, format = Binary)]
#[allow(dead_code)]
pub struct MeshFileBytes(Vec<u8>);

/// What the converter can do, for a client to check before uploading.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Model3dConvertFormats {
    /// Containers accepted as both `from` and `to`. Every pair converts, in
    /// both directions. Grows with litegen's converter, so read it rather than
    /// hardcoding it; it can be narrower than the `Model3dFormat` vocabulary.
    #[schema(value_type = Vec<crate::types::Model3dFormat>)]
    pub formats: Vec<String>,
    /// Largest request body accepted, in bytes. Larger bodies get `413`.
    pub max_input_bytes: u64,
}

/// GET /v1/utils/models3d/formats — Containers the standalone converter reads and writes.
#[utoipa::path(
    get,
    path = "/v1/utils/models3d/formats",
    responses(
        (status = 200, description = "Supported containers and the input size cap", body = Model3dConvertFormats),
    ),
    tag = "Utilities"
)]
pub async fn list_model3d_convert_formats() -> Json<Model3dConvertFormats> {
    Json(Model3dConvertFormats {
        formats: derivable_formats(),
        max_input_bytes: mesh::MAX_INPUT_BYTES as u64,
    })
}

/// POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.
///
/// The body is the raw file. The response is the converted file, with the
/// target's `Content-Type`. Whatever the target container can hold survives
/// (materials, textures, skins, animation); what it cannot is dropped and
/// named in `X-Litegen-Conversion-Losses`.
///
/// Live: `curl --data-binary @fox.glb "https://app.litegen.ai/api/v1/utils/models3d/convert?from=glb&to=obj" -H "Authorization: Bearer sk_live_..." -o fox.obj`
#[utoipa::path(
    post,
    path = "/v1/utils/models3d/convert",
    params(
        ("from" = crate::types::Model3dFormat, Query, description = "Container the request body is in. Must be one of `GET /v1/utils/models3d/formats`."),
        ("to" = crate::types::Model3dFormat, Query, description = "Container to convert to. Must be one of `GET /v1/utils/models3d/formats`."),
    ),
    request_body(content = inline(MeshFileBytes), content_type = "application/octet-stream", description = "The mesh file, raw. Any Content-Type is accepted; `from` decides how it is read."),
    responses(
        (status = 200, description = "The converted file. Content-Type is the target container's, e.g. model/obj", content_type = "application/octet-stream", body = inline(MeshFileBytes),
            headers(("X-Litegen-Conversion-Losses" = String, description = "Comma-separated kinds of content in the input that the target container could not hold, sorted and deduplicated: animation, skins, morph_targets, normals, uvs, vertex_colors, textures, image_formats, materials, alpha, hierarchy, meshes, texture_sampling, texture_transforms, geometry. Judged from what the input actually contains, so a plain mesh to STL loses nothing. Absent when nothing was lost."))),
        (status = 400, description = "`from` or `to` missing or not convertible (`param_required`, `format_unsupported`), or the body is not a `from` file (`format_mismatch`)", body = ErrorResponse),
        (status = 413, description = "Body over `max_input_bytes` (`body_too_large`)", body = ErrorResponse),
        (status = 422, description = "The file parsed as `from` but cannot be converted (`mesh_malformed`, `mesh_unsupported`, `mesh_empty`)", body = ErrorResponse),
        (status = 503, description = "Server at capacity, retry shortly (`server_at_capacity`)", body = ErrorResponse),
    ),
    tag = "Utilities"
)]
pub async fn convert_model3d(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ConvertQuery>,
    body: Body,
) -> Response {
    // Cheap checks first, so a bad query never costs a slot or an upload.
    let from = match parse_format(q.from.as_deref(), "from") {
        Ok(f) => f,
        Err(r) => return r,
    };
    let to = match parse_format(q.to.as_deref(), "to") {
        Ok(f) => f,
        Err(r) => return r,
    };

    // The same slot the Validated* extractors take, and for the same reason:
    // taken before buffering, it bounds how many bodies are in memory at once.
    let Some(permit) = state.in_flight.try_acquire_owned() else {
        let mut resp = reject(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_at_capacity",
            "server at capacity, retry shortly".into(),
            None,
        );
        resp.headers_mut()
            .insert(header::RETRY_AFTER, axum::http::HeaderValue::from_static("1"));
        return resp;
    };

    // Raw `Body` bypasses axum's DefaultBodyLimit, so this cap is the limit.
    // It matches the converter's own, so anything litegen can derive during a
    // generation it can also convert here.
    let bytes = match axum::body::to_bytes(body, mesh::MAX_INPUT_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            return reject(
                StatusCode::PAYLOAD_TOO_LARGE,
                "body_too_large",
                format!("body is over the {}-byte limit", mesh::MAX_INPUT_BYTES),
                None,
            )
        }
    };
    let input_len = bytes.len();

    // CPU-bound on untrusted bytes: off the runtime thread. The permit moves in
    // with the work, so a client that disconnects mid-conversion does not free
    // its slot while the CPU is still busy on its file.
    let started = std::time::Instant::now();
    // Self-contained, so the package is exactly one file: sidecars a target
    // would normally reference (OBJ's .mtl, Collada's images) are embedded or
    // reported as losses instead of silently missing from a single download.
    let result = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        mesh::convert_scene(&bytes, from, &mesh::NoSidecars, to, mesh::WriteOptions { self_contained: true })
    })
    .await;

    let failed = |detail: &dyn std::fmt::Display| {
        tracing::error!(from = %from, to = %to, input_bytes = input_len, error = %detail, "mesh conversion failed");
        reject(
            StatusCode::INTERNAL_SERVER_ERROR,
            "conversion_failed",
            "the converter failed on this file".into(),
            None,
        )
    };
    let written = match result {
        Ok(Ok(w)) => w,
        Ok(Err(e)) => return mesh_error_response(e, from),
        Err(e) => return failed(&e),
    };
    let losses = loss_kinds(&written.losses);
    let Some(out) = written.package.files.into_iter().next() else {
        return failed(&"converter returned an empty package");
    };

    tracing::info!(
        from = %from,
        to = %to,
        input_bytes = input_len,
        output_bytes = out.bytes.len(),
        losses = %losses,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "converted mesh",
    );

    let content_type =
        crate::proxy::storage::model3d_content_type(to.as_str(), "application/octet-stream").to_string();
    let mut resp = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"model.{to}\"")),
        ],
        out.bytes,
    )
        .into_response();
    // Omitted rather than empty when nothing was lost, like X-Litegen-Dropped-Params.
    if !losses.is_empty() {
        if let Ok(v) = axum::http::HeaderValue::from_str(&losses) {
            resp.headers_mut().insert(LOSSES_HEADER, v);
        }
    }
    resp
}

const LOSSES_HEADER: &str = "x-litegen-conversion-losses";

/// The header value: loss kinds, alphabetical and deduplicated, no spaces.
/// Only the kinds go on the wire. Their spellings are a contract pinned in
/// `mesh`; `what` and `why` are prose that may change.
fn loss_kinds(losses: &[mesh::Loss]) -> String {
    let kinds: std::collections::BTreeSet<&'static str> = losses.iter().map(|l| l.kind.as_str()).collect();
    kinds.into_iter().collect::<Vec<_>>().join(",")
}

/// `from`/`to` against what the converter can do, which is the authority here
/// rather than the `Model3dFormat` vocabulary: the two can differ, and a word
/// that is in the vocabulary but not the converter gets an honest 400.
fn parse_format(value: Option<&str>, param: &str) -> Result<MeshFormat, Response> {
    let supported = || derivable_formats().join(", ");
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Err(reject(
            StatusCode::BAD_REQUEST,
            "param_required",
            format!("`{param}` is required; one of: {}", supported()),
            Some(param),
        ));
    };
    MeshFormat::parse(raw).ok_or_else(|| {
        reject(
            StatusCode::BAD_REQUEST,
            "format_unsupported",
            format!("`{param}` '{raw}' cannot be converted; supported: {}", supported()),
            Some(param),
        )
    })
}

/// Every `MeshError` is about the caller's file, so none is a 5xx. A file that
/// is not `from` at all is the caller naming the wrong format (400, pointing
/// at `from`); one that is `from` but unusable is unprocessable (422).
fn mesh_error_response(e: MeshError, from: MeshFormat) -> Response {
    let message = e.to_string();
    match e {
        MeshError::NotThisFormat { .. } => reject(
            StatusCode::BAD_REQUEST,
            "format_mismatch",
            format!("{message}; check that `from` ({from}) matches the file"),
            Some("from"),
        ),
        MeshError::Malformed(_) => reject(StatusCode::UNPROCESSABLE_ENTITY, "mesh_malformed", message, None),
        MeshError::Unsupported(_) => reject(StatusCode::UNPROCESSABLE_ENTITY, "mesh_unsupported", message, None),
        MeshError::Empty => reject(StatusCode::UNPROCESSABLE_ENTITY, "mesh_empty", message, None),
    }
}

fn reject(status: StatusCode, code: &str, message: String, param: Option<&str>) -> Response {
    let r#type = match status {
        StatusCode::INTERNAL_SERVER_ERROR => "internal_error",
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
        _ => "validation_error",
    };
    (
        status,
        Json(ErrorResponse {
            error: ErrorDetail {
                message,
                r#type: r#type.to_string(),
                code: Some(code.to_string()),
                provider_error: None,
                param: param.map(str::to_string),
                model: None,
            },
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_format_parses_and_nothing_else_does() {
        for f in derivable_formats() {
            assert!(parse_format(Some(&f), "to").is_ok(), "{f}");
            assert!(parse_format(Some(&f.to_ascii_uppercase()), "to").is_ok(), "{f} upper-case");
        }
        // In the vocabulary, not in the converter: the 400 is the honest answer.
        for f in ["fbx", "usdz", "png", ""] {
            if MeshFormat::parse(f).is_none() {
                assert!(parse_format(Some(f), "to").is_err(), "{f:?}");
            }
        }
        assert!(parse_format(None, "from").is_err());
    }

    #[test]
    fn the_losses_header_is_kinds_only_sorted_and_deduplicated() {
        use mesh::{Loss, LossKind};
        let losses = [
            Loss::new(LossKind::Uvs, "uv set 1", "stl has no texture coordinates"),
            Loss::new(LossKind::Animation, "clip 'bend'", "stl is static"),
            Loss::new(LossKind::Uvs, "uv set 0", "stl has no texture coordinates"),
            Loss::new(LossKind::MorphTargets, "smile", "stl is static"),
        ];
        assert_eq!(loss_kinds(&losses), "animation,morph_targets,uvs");
        assert_eq!(loss_kinds(&[]), "", "empty, so the handler omits the header");
    }

    #[test]
    fn mesh_errors_are_never_server_errors() {
        let cases = [
            MeshError::NotThisFormat { expected: MeshFormat::Glb, detail: "bad magic".into() },
            MeshError::Malformed("x".into()),
            MeshError::Unsupported("x".into()),
            MeshError::Empty,
        ];
        for e in cases {
            let status = mesh_error_response(e.clone(), MeshFormat::Glb).status();
            assert!(status.is_client_error(), "{e:?} -> {status}");
        }
    }
}
