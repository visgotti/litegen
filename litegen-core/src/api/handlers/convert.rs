//! Standalone mesh conversion: [`crate::mesh`] as a utility, with no generation.
//!
//! The generation path already converts. When a vendor returns only GLB,
//! delivery (`proxy::model3d_deliver`) derives the other containers the
//! caller asked for.
//! These endpoints expose the same converter on its own, for a mesh the caller
//! already has: one from an earlier generation, or one that never came from
//! litegen. No provider is called, no generation row is written and nothing is
//! billed.
//!
//! The body is untrusted and goes straight into the mesh parsers, so the
//! handler takes a conversion slot (its own pool, never a generation's) before
//! the body is buffered, caps the body's size and how long it may take to
//! arrive, and parses on a blocking thread.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::api::middleware::body::{BodyBudget, BodyError, Buffered};
use crate::api::middleware::AppState;
use crate::mesh::{self, MeshError, MeshFormat};
use crate::proxy::model3d_convert::derivable_formats;
use crate::types::{ErrorDetail, ErrorResponse};

/// The query parameters the converter takes. Anything else is refused, so a
/// caller learns an option does not exist rather than having it ignored.
const PARAMS: [&str; 14] = [
    "from", "to", "units", "up", "from_units", "from_up", "fbx_color_space", "package", "ktx2_textures", "fbx_light_units", "usd_light_units", "dae_morphs", "usdz_root",
    "usd_clips",
];

/// Containers with no units or up axis of their own, whose numbers mean what
/// the caller says they mean: `units`/`up` (writing) and
/// `from_units`/`from_up` (reading) apply to these alone.
const UNITLESS: [MeshFormat; 3] = [MeshFormat::Stl, MeshFormat::Obj, MeshFormat::Ply];

/// A parsed request: the containers, and the frame (metres per unit, +Z up)
/// to read the input as and to write the output in, when not glTF's.
#[derive(Clone, Copy)]
struct ConvertQuery {
    from: MeshFormat,
    to: MeshFormat,
    read_as: Option<(f64, bool)>,
    write_as: Option<(f64, bool)>,
    /// How FBX material and light colours are stored, at whichever end is FBX.
    fbx_colors: mesh::FbxColorSpace,
    /// The units FBX light strengths are stored in, at whichever end is FBX.
    fbx_light_units: mesh::LightUnits,
    /// The units USD light strengths are stored in, at whichever end is usdz,
    /// usda or usdc.
    usd_light_units: mesh::LightUnits,
    /// How a Collada file's morph targets are written, when `to` is dae.
    dae_morphs: mesh::DaeMorphs,
    /// What the root layer of the usdz written is, when `to` is usdz.
    usdz_root: mesh::UsdzRoot,
    /// Which clips the USD file written holds, when `to` is usda, usdc or
    /// usdz; `None` is the container's default.
    usd_clips: Option<mesh::UsdClips>,
    package: Packaging,
    ktx2_textures: Ktx2Textures,
}

/// What the converter does with a KTX2 (Basis Universal) texture
/// (`ktx2_textures`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, utoipa::ToSchema)]
#[schema(rename_all = "snake_case")]
enum Ktx2Textures {
    /// PNG in every container: the default.
    Png,
    /// A glb or gltf keeps the KTX2 through `KHR_texture_basisu`; any other
    /// container gets PNG.
    Keep,
}

/// What the converter answers with (`package`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, utoipa::ToSchema)]
#[schema(rename_all = "snake_case")]
enum Packaging {
    /// The converted file alone, as one download: the default.
    File,
    /// A zip of the converted file and the sidecars it references.
    Zip,
}

/// How long an upload may take. A slow client must not hold a conversion
/// slot open indefinitely.
const BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The slowest average download a converted file is sent at, after the
/// [`BODY_TIMEOUT`] grace. A reader slower than this loses the transfer, so it
/// cannot pin its slot, or the file the slot accounts for, indefinitely.
const MIN_SEND_BYTES_PER_SEC: usize = 1024 * 1024;

/// The converted file goes to the connection in pieces this size, each a
/// copy: a stalled connection then holds a piece or two, never the file.
const SEND_CHUNK: usize = 64 * 1024;

fn send_limit(len: usize) -> std::time::Duration {
    BODY_TIMEOUT + std::time::Duration::from_secs((len / MIN_SEND_BYTES_PER_SEC) as u64)
}

/// Conversions in flight at once, process-wide. CPU-bound work on untrusted
/// bytes has slots of its own: taking the generation endpoints'
/// (`AppState::in_flight`, shared by every tenant) let unbilled uploads answer
/// every tenant's generations with 503. A slot is taken once the upload has
/// arrived (the uploads still arriving are bounded by [`conversion_budget`])
/// and held until the converted file is sent ([`slot_held_body`]), so this
/// caps how many conversions, and how many converted files, are in memory at
/// once. One conversion peaks at 5–13 times
/// its input (measured on vendor meshes, `examples/mesh_peak.rs`): about
/// 1.7 GB per slot at the input cap.
fn conversion_slots() -> Arc<Semaphore> {
    static SLOTS: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Arc::new(Semaphore::new(slot_count()))).clone()
}

/// How many conversions run at once: one per core, 2 to 8.
fn slot_count() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8)
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
    /// both directions. Today every `Model3dFormat`; it follows litegen's
    /// converter, so read it rather than hardcoding it.
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
/// named in `X-Litegen-Conversion-Losses`. WebP and KTX2 (Basis Universal)
/// textures are re-encoded as PNG, as generations deliver them, so no
/// container drops them and, by default, no glTF requires
/// `EXT_texture_webp` or `KHR_texture_basisu` (`ktx2_textures=keep` leaves a
/// KTX2 in a glb or gltf, which then requires `KHR_texture_basisu`). By
/// default the output is one file: OBJ carries its materials as vertex
/// colours, and Collada, USDA and USDC leave textures out (named as losses).
/// `package=zip` answers with a zip of the file and its sidecars instead,
/// which keeps them. A USD file (`.usda`, `.usdc`, or a `.usd` holding either)
/// is read with `from=usda` or `from=usdc`, whichever encoding it is in.
///
/// Live: `curl --data-binary @fox.glb "https://app.litegen.ai/api/v1/utils/models3d/convert?from=glb&to=obj" -H "Authorization: Bearer sk_live_..." -o fox.obj`
#[utoipa::path(
    post,
    path = "/v1/utils/models3d/convert",
    params(
        ("from" = crate::types::Model3dFormat, Query, description = "Container the request body is in. Must be one of `GET /v1/utils/models3d/formats`. `usda` and `usdc` each read either USD encoding, text or a binary crate (`.usdc`, or a `.usd` holding either)."),
        ("to" = crate::types::Model3dFormat, Query, description = "Container to convert to. Must be one of `GET /v1/utils/models3d/formats`."),
        ("units" = Option<String>, Query, description = "Only when `to` is `stl`, `obj` or `ply`, which carry no units: the unit the written numbers are in, `m` (the default), `cm`, `mm`, `in` or `ft`. Slicers read STL as millimetres."),
        ("up" = Option<String>, Query, description = "Only when `to` is `stl`, `obj` or `ply`: the written up axis, `y` (the default, glTF's) or `z` (slicers, CAD, Blender), turning +Y up to +Z up and the front (+Z) to -Y."),
        ("from_units" = Option<String>, Query, description = "Only when `from` is `stl`, `obj` or `ply`: the unit the body's numbers are in, as `units`. Default `m`."),
        ("from_up" = Option<String>, Query, description = "Only when `from` is `stl`, `obj` or `ply`: the body's up axis, `y` (the default) or `z`."),
        ("fbx_color_space" = Option<String>, Query, description = "Only when `from` or `to` is `fbx`: how FBX material and light colours are stored, at that end. `srgb` (the default; three.js reads them so) or `linear` (Blender, ufbx and assimp read both so, and Unity and Unreal material colours). Vertex colours are sRGB either way."),
        ("package" = inline(Option<Packaging>), Query, description = "`file` (the default) answers with the converted file alone; `zip` answers with a zip (`application/zip`, `model.<to>.zip`) of the file and the sidecars it references (an OBJ's `.mtl` and `textures/`, Collada's, USDA's and USDC's `textures/`, a `.gltf`'s `.bin` and images), at the paths it names them by, the file first. Unzipped, the folder opens as it is: an OBJ keeps its materials, Collada, USDA and USDC keep their textures, and the losses header is judged against the package. A single-file container (glb, fbx, stl, ply, 3mf, usdz) zips to one entry, the `file` answer byte for byte."),
        ("ktx2_textures" = inline(Option<Ktx2Textures>), Query, description = "`png` (the default) re-encodes every KTX2 (Basis Universal) texture as PNG, as WebP is, so no container drops it and no glTF requires `KHR_texture_basisu`: ETC1S, UASTC and uncompressed RGBA8 KTX2 files decode, and the PNG holds the values the KTX2 stores, with no colour conversion. A KTX2 that cannot be decoded, or that would pass the 64 MiB texture budget, is left out and named as an `image_formats` loss. `keep` leaves the KTX2 as it is, byte for byte, when `to` is `glb` or `gltf`, referenced through `KHR_texture_basisu`, which the file then requires, so a reader with no KTX2 support may refuse it: it suits viewers that upload KTX2 to the GPU (three.js with KTX2Loader, Babylon.js, Godot). Every other `to` gets PNG whichever you choose."),
        ("fbx_light_units" = Option<String>, Query, description = "Only when `from` or `to` is `fbx`: the units FBX light strengths are stored in, at that end. `photometric` (the default) is the scene's own, glTF's: `Intensity` is 100 × the candela (point and spot lights) or lux (directional lights), which three.js's FBXLoader and ufbx read back. `blender` stores 100 × the `energy` Blender's own glTF importer gives the same scene's glTF light (a point or spot light's candela × 4π / 683 W, a directional light's lux / 683 W/m²), so the light opens in Blender as bright as the glTF does, and reads a light Blender wrote as the glTF light whose Blender glTF import gives that `energy`. Colour, cones and range are the same either way. The option applies at whichever end is fbx, so `from=fbx&to=fbx` with `blender` reads Blender's units and writes them back, a no-op: to move a Blender file to `photometric`, convert twice (fbx to glb with `blender`, then glb to fbx)."),
        ("usd_light_units" = Option<String>, Query, description = "Only when `from` or `to` is `usdz`, `usda` or `usdc`: the units USD light strengths are stored in, at that end. `photometric` (the default) is UsdLux's own, by the schema: a point or spot light is a normalised `SphereLight` of 4 × the candela, a directional light a normalised `DistantLight` of the lux (Storm, usdview's renderer, agrees on the sun and draws such a sphere 4× brighter). `blender` writes the UsdLux values Blender's USD exporter writes for the `energy` Blender's own glTF importer gives the same scene's glTF light (a point or spot light's candela × 4π / 683 W, a directional light's lux / 683 W/m²), so the light opens in Blender as bright as the glTF does, and reads a light prim without litegen's `customData` (every one Blender wrote) as the glTF light whose Blender glTF import gives that `energy`. A file litegen wrote also carries its exact strengths in `customData`, which litegen reads in preference to the UsdLux values under either choice. The option applies at whichever end is usdz, usda or usdc: `from=usdz&to=usdz` with `blender` keeps the strengths of a file in Blender's units, and rewrites those of a file litegen wrote in Blender's units, its `customData` keeping the exact values. To move a Blender file to `photometric`, convert twice (usdz to glb with `blender`, then glb to usdz)."),
        ("dae_morphs" = Option<String>, Query, description = "Only when `to` is `dae`: how Collada's morph targets (blend shapes) are written. `bake` (the default) applies the targets at each node's weights and drops their weight animation, which every Collada reader opens. `keep` writes them as `<morph>` controllers with their weight animation, which SceneKit (and so Xcode) and Godot read, while three.js's ColladaLoader and assimp refuse the whole file, so choose it only for a reader that opens controllers."),
        ("usdz_root" = Option<String>, Query, description = "Only when `to` is `usdz`: what the package's root layer, its first entry, is. `usda` (the default) is USDA text, which `usdchecker --arkit` accepts (the rule demanding a crate was removed in OpenUSD 24.03) and which `<model-viewer>` hands to AR Quick Look as it is. `usdc` is the binary crate, as Apple's own packagers write it: the package is `model.usdc` and its textures, every entry stored and 64-byte aligned either way. The two read back as one scene."),
        ("usd_clips" = Option<String>, Query, description = "Only when `to` is `usda`, `usdc` or `usdz`: which animation clips the USD file holds. `all` keeps every clip as an alternative, a variant set `Animation` on the default prim with one variant per clip, the first selected, so a reader that composes variants can switch between them and one that does not plays the first. Every clip's skeletal and blend-shape animation is kept; rigid (node) motion is the first clip's only, the other clips' is named in the losses, and a scene whose clips are all rigid writes as `first`. `first` keeps the first clip alone. The default is `all` for `usda` and `usdc`, `first` for `usdz` until AR Quick Look's behaviour with variants is checked on a device."),
    ),
    request_body(content = inline(MeshFileBytes), content_type = "application/octet-stream", description = "The mesh file, raw. Any Content-Type is accepted; `from` decides how it is read."),
    responses(
        (status = 200, description = "The converted file, with the target container's Content-Type (e.g. model/obj); with `package=zip`, a zip of it and its sidecars (`application/zip`)", content((inline(MeshFileBytes) = "application/octet-stream"), (inline(MeshFileBytes) = "application/zip")),
            headers(("X-Litegen-Conversion-Warnings" = String, description = "What reading the input itself dropped or approximated, in prose, joined by `; `: a material library or texture the upload could not include, a glTF extension litegen does not carry (material variants, animation pointers, …), cubic and NURBS curves, a Gaussian splat's extra attributes, an FBX animation layer. Non-ASCII characters are replaced by `?`. Absent when the reader had nothing to say."),
                ("X-Litegen-Conversion-Losses" = String, description = "Comma-separated kinds of content in the input that the target container could not hold, in alphabetical order and deduplicated: animation, skins, morph_targets, normals, uvs, vertex_colors, textures, image_formats, materials, alpha, hierarchy, meshes, texture_sampling, texture_transforms, geometry, lights, cameras, points, lines. Judged from what the input actually contains: a mesh with no normals, materials or UVs loses nothing to STL, while one that has them names them. `points` and `lines` name point and line primitives (a point cloud, a wireframe) in a container that has none: STL, 3MF and FBX hold neither, Collada holds no points, and PLY holds points only in a file with no faces or edges. Absent when nothing was lost."))),
        (status = 400, description = "`from` or `to` missing, repeated or not convertible, an unknown parameter, or `units`/`up`/`from_units`/`from_up` for a container that declares its own, or `fbx_color_space` or `fbx_light_units` with no fbx end, or `usd_light_units` with no usdz, usda or usdc end, or `dae_morphs` when `to` is not dae, or `usdz_root` when `to` is not usdz, or `usd_clips` when `to` is not usda, usdc or usdz (`param_required`, `param_repeated`, `param_unsupported`, `format_unsupported`); a `units`, `up`, `fbx_color_space`, `fbx_light_units`, `usd_light_units`, `dae_morphs`, `usdz_root`, `usd_clips`, `package` or `ktx2_textures` value the converter does not take (`param_invalid`); the body is not a `from` file (`format_mismatch`); or the upload broke off (`body_unreadable`)", body = ErrorResponse),
        (status = 408, description = "The upload took longer than 120 s (`body_timeout`)", body = ErrorResponse),
        (status = 413, description = "Body over `max_input_bytes` (`body_too_large`)", body = ErrorResponse),
        (status = 422, description = "The file is `from` but unreadable or past a converter limit (`mesh_malformed`, `mesh_unsupported`, `mesh_empty`)", body = ErrorResponse),
        (status = 500, description = "The converter failed on this file: a litegen bug, logged (`conversion_failed`)", body = ErrorResponse),
        (status = 503, description = "All conversion slots are busy, or too many uploads are arriving at once; retry shortly (`server_at_capacity`, with `Retry-After`). Conversions have slots of their own and never take a generation's; a slot is taken once the upload has arrived", body = ErrorResponse),
    ),
    tag = "Utilities"
)]
pub async fn convert_model3d(
    State(_state): State<Arc<AppState>>,
    Query(pairs): Query<Vec<(String, String)>>,
    body: Body,
) -> Response {
    convert(&pairs, body).await
}

async fn convert(pairs: &[(String, String)], body: Body) -> Response {
    convert_in(conversion_slots(), pairs, body).await
}

/// [`convert`], taking its slot from `slots`.
async fn convert_in(slots: Arc<Semaphore>, pairs: &[(String, String)], body: Body) -> Response {
    // Cheap checks first, so a bad query never costs a slot or an upload.
    let query = match parse_query(pairs) {
        Ok(q) => q,
        Err(r) => return r,
    };
    let ConvertQuery { from, to, package, .. } = query;
    let at_capacity = || {
        let mut resp = reject(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_at_capacity",
            "server at capacity, retry shortly".into(),
            None,
        );
        resp.headers_mut()
            .insert(header::RETRY_AFTER, axum::http::HeaderValue::from_static("1"));
        resp
    };

    // With every slot busy, refuse before the upload rather than after it.
    if slots.available_permits() == 0 {
        return at_capacity();
    }
    // The upload first, charged to the converter's budget as it arrives: a
    // client that uploads slowly holds the bytes it sent, never a slot. The
    // budget bounds the uploads in memory as the slots did.
    let upload = match read_body(body).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    // Then the slot, held until the converted file is sent.
    let Ok(permit): Result<OwnedSemaphorePermit, _> = slots.try_acquire_owned() else {
        return at_capacity();
    };
    let input_len = upload.bytes.len();

    // CPU-bound on untrusted bytes: off the runtime thread. The permit moves in
    // with the work, so a client that disconnects mid-conversion does not free
    // its slot while the CPU is still busy on its file, and it comes back out
    // with the result, to be held until that file is sent.
    let started = std::time::Instant::now();
    // One download either way. As one file, sidecars a target would normally
    // reference (OBJ's .mtl, Collada's images) are embedded or reported as
    // losses instead of silently missing; as a zip, they travel inside it.
    let result = tokio::task::spawn_blocking(move || {
        let converted = convert_bytes(&upload.bytes, &query);
        // The upload and its share of the budget go now, not with the reply.
        drop(upload);
        (converted, permit)
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
    let ((written, warnings), permit) = match result {
        Ok((Ok(w), permit)) => (w, permit),
        Ok((Err(e), _)) => return mesh_error_response(e, from),
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
        zip = package == Packaging::Zip,
        losses = %losses,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "converted mesh",
    );

    let (content_type, filename) = match package {
        Packaging::File => (crate::proxy::storage::model3d_content_type(to.as_str(), "application/octet-stream"), format!("model.{to}")),
        Packaging::Zip => (crate::proxy::storage::model3d_content_type("zip", "application/octet-stream"), format!("model.{to}.zip")),
    };
    let len = out.bytes.len();
    let mut resp = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{filename}\"")),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        slot_held_body(out.bytes, permit, send_limit(len)),
    )
        .into_response();
    // Omitted rather than empty when nothing was lost, like X-Litegen-Dropped-Params.
    if !losses.is_empty() {
        if let Ok(v) = axum::http::HeaderValue::from_str(&losses) {
            resp.headers_mut().insert(LOSSES_HEADER, v);
        }
    }
    if !warnings.is_empty() {
        if let Ok(v) = axum::http::HeaderValue::from_str(&warnings_header(&warnings)) {
            resp.headers_mut().insert(WARNINGS_HEADER, v);
        }
    }
    resp
}

const LOSSES_HEADER: &str = "x-litegen-conversion-losses";

/// The converted file as a response body that keeps its conversion slot
/// until the connection has taken the last byte, or the client hangs up.
///
/// A task owns the file and the permit, and hands the file to the body a
/// [`SEND_CHUNK`] at a time through a one-slot channel, so the connection's
/// own flow control paces it. When `limit` passes first, the task drops the
/// unsent rest and the permit at once, whether or not the connection is still
/// polling the body, and the body then ends with an error. With the
/// `Content-Length` the handler sets, the client sees a failed transfer, never
/// a short file.
fn slot_held_body(file: Vec<u8>, permit: OwnedSemaphorePermit, limit: std::time::Duration) -> Body {
    let total = file.len();
    let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(1);
    tokio::spawn(async move {
        let deadline = tokio::time::sleep(limit);
        tokio::pin!(deadline);
        let mut expired = false;
        for chunk in file.chunks(SEND_CHUNK) {
            tokio::select! {
                sent = tx.send(bytes::Bytes::copy_from_slice(chunk)) => {
                    if sent.is_err() {
                        break; // the body was dropped: the client went away
                    }
                }
                () = &mut deadline => {
                    expired = true;
                    break;
                }
            }
        }
        drop(file);
        if expired {
            tracing::warn!(bytes = total, limit_s = limit.as_secs(), "converted file not taken in time; transfer dropped");
        } else {
            // Every piece is queued: the slot is held until the body is done
            // with them, or the deadline.
            tokio::select! {
                () = tx.closed() => {}
                () = &mut deadline => {}
            }
        }
        drop(permit);
    });
    // The body ends at its last byte, without waiting for the channel to
    // close: the task holding the sender waits for this body to be dropped.
    let pieces = futures::stream::unfold((rx, 0usize), move |(mut rx, sent)| async move {
        if sent == total {
            return None;
        }
        match rx.recv().await {
            Some(piece) => {
                let n = piece.len();
                Some((Ok(piece), (rx, sent + n)))
            }
            None if sent < total => Some((
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the converted file was not taken in time")),
                (rx, total),
            )),
            None => None,
        }
    });
    Body::from_stream(pieces)
}

/// Read, normalise textures, write. A WebP or KTX2 (Basis Universal) texture
/// is re-encoded as PNG first, as generation delivery does
/// (`normalize::transcode_textures`): no container then drops it, and, by
/// default, no glTF written requires `EXT_texture_webp` or
/// `KHR_texture_basisu`. One that cannot be decoded (a broken WebP or KTX2),
/// or that would pass the texture budget, is left out and reported. With
/// `ktx2_textures=keep` a glb or gltf output keeps its KTX2 images, and so
/// requires `KHR_texture_basisu` (WebP is still re-encoded). With
/// [`Packaging::Zip`] the scene is written as a package, and the package's one
/// file is its zip.
fn convert_bytes(bytes: &[u8], query: &ConvertQuery) -> Result<(mesh::SceneWrite, Vec<String>), MeshError> {
    let ConvertQuery { from, to, read_as, write_as, fbx_colors, fbx_light_units, usd_light_units, dae_morphs, usdz_root, usd_clips, package, ktx2_textures } = *query;
    let read = mesh::read_scene_with(
        bytes,
        from,
        &mesh::NoSidecars,
        mesh::ReadOptions { fbx_color_space: fbx_colors, fbx_light_units, usd_light_units },
    )?;
    let warnings = read.warnings;
    let mut scene = read.scene;
    if let Some((unit_m, z_up)) = read_as {
        mesh::normalize::read_frame(&mut scene, unit_m, z_up);
    }
    if let Some((unit_m, z_up)) = write_as {
        mesh::normalize::express_frame(&mut scene, unit_m, z_up);
    }
    let keeps_ktx2 = ktx2_textures == Ktx2Textures::Keep && matches!(to, MeshFormat::Glb | MeshFormat::Gltf);
    let transcoded = if keeps_ktx2 {
        mesh::normalize::transcode_textures_keeping_ktx2(&mut scene)
    } else {
        mesh::normalize::transcode_textures(&mut scene)
    };
    let self_contained = package == Packaging::File;
    let mut written = mesh::write_scene(
        &scene,
        to,
        mesh::WriteOptions { self_contained, fbx_color_space: fbx_colors, fbx_light_units, usd_light_units, dae_morphs, usdz_root, usd_clips, ..Default::default() },
    )?;
    for mesh::normalize::Undecodable { texture: t, reason: why } in &transcoded.dropped {
        let reason = if why.is_empty() { String::new() } else { format!(": {why}") };
        written.losses.push(mesh::Loss::new(
            mesh::LossKind::ImageFormats,
            format!("texture {t}"),
            format!("litegen could not decode it to PNG for this container{reason}"),
        ));
    }
    for t in &transcoded.over_budget {
        written.losses.push(mesh::Loss::new(mesh::LossKind::ImageFormats, format!("texture {t}"), mesh::normalize::OVER_BUDGET));
    }
    if package == Packaging::Zip {
        written.package = mesh::Package::single(format!("model.{to}.zip"), mesh::zip::write_package(&written.package)?);
    }
    Ok((written, warnings))
}

const WARNINGS_HEADER: &str = "x-litegen-conversion-warnings";

/// The reader's warnings as one header value: joined by `"; "`, anything
/// outside visible ASCII replaced by `?` (header values carry no UTF-8),
/// cut at 4 KiB.
fn warnings_header(warnings: &[String]) -> String {
    let joined = warnings.join("; ");
    let clean: String = joined.chars().map(|c| if c == ' ' || c.is_ascii_graphic() { c } else { '?' }).collect();
    if clean.len() > 4096 {
        format!("{}...", &clean[..4093])
    } else {
        clean
    }
}

/// `from` and `to`, each exactly once, and nothing else.
fn parse_query(pairs: &[(String, String)]) -> Result<ConvertQuery, Response> {
    for (i, (k, _)) in pairs.iter().enumerate() {
        if !PARAMS.contains(&k.as_str()) {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "param_unsupported",
                format!("`{k}` is not a parameter of the converter; it takes {}", PARAMS.join(", ")),
                Some(k),
            ));
        }
        if pairs[..i].iter().any(|(earlier, _)| earlier == k) {
            return Err(reject(StatusCode::BAD_REQUEST, "param_repeated", format!("`{k}` is given more than once"), Some(k)));
        }
    }
    let value = |name: &str| pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let (from, to) = (parse_format(value("from"), "from")?, parse_format(value("to"), "to")?);
    // A frame for the side that has no units of its own: `side` is `from` or
    // `to`, `format` its container.
    let frame = |units: &str, up: &str, side: &str, format: MeshFormat| -> Result<Option<(f64, bool)>, Response> {
        let (u, a) = (value(units), value(up));
        if u.is_none() && a.is_none() {
            return Ok(None);
        }
        if !UNITLESS.contains(&format) {
            let name = if u.is_some() { units } else { up };
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "param_unsupported",
                format!("`{name}` applies when `{side}` is stl, obj or ply, which carry no units or up axis; {format} declares its own"),
                Some(name),
            ));
        }
        let metres = match u {
            None => 1.0,
            Some(v) => mesh::normalize::metres_per_unit(v).ok_or_else(|| {
                reject(StatusCode::BAD_REQUEST, "param_invalid", format!("`{units}` is {v:?}; it takes m, cm, mm, in or ft"), Some(units))
            })?,
        };
        let z_up = match a.map(|v| v.trim().to_ascii_lowercase()) {
            None => false,
            Some(v) if v == "y" || v == "+y" => false,
            Some(v) if v == "z" || v == "+z" => true,
            Some(v) => return Err(reject(StatusCode::BAD_REQUEST, "param_invalid", format!("`{up}` is {v:?}; it takes y or z"), Some(up))),
        };
        Ok(Some((metres, z_up)))
    };
    let read_as = frame("from_units", "from_up", "from", from)?;
    let write_as = frame("units", "up", "to", to)?;
    let fbx_colors = match value("fbx_color_space") {
        None => mesh::FbxColorSpace::default(),
        Some(_) if from != MeshFormat::Fbx && to != MeshFormat::Fbx => {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "param_unsupported",
                "`fbx_color_space` applies when `from` or `to` is fbx".into(),
                Some("fbx_color_space"),
            ))
        }
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "srgb" => mesh::FbxColorSpace::Srgb,
            "linear" => mesh::FbxColorSpace::Linear,
            _ => {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "param_invalid",
                    format!("`fbx_color_space` is {v:?}; it takes srgb or linear"),
                    Some("fbx_color_space"),
                ))
            }
        },
    };
    // `fbx_light_units` and `usd_light_units`: refused where no end is that
    // format, as `fbx_color_space` is; `ends` names the formats for the text.
    let light_units = |name: &str, applies: bool, ends: &str| -> Result<mesh::LightUnits, Response> {
        match value(name) {
            None => Ok(mesh::LightUnits::default()),
            Some(_) if !applies => Err(reject(
                StatusCode::BAD_REQUEST,
                "param_unsupported",
                format!("`{name}` applies when `from` or `to` is {ends}"),
                Some(name),
            )),
            Some(v) => match v.trim().to_ascii_lowercase().as_str() {
                "photometric" => Ok(mesh::LightUnits::Photometric),
                "blender" => Ok(mesh::LightUnits::Blender),
                _ => Err(reject(
                    StatusCode::BAD_REQUEST,
                    "param_invalid",
                    format!("`{name}` is {v:?}; it takes photometric or blender"),
                    Some(name),
                )),
            },
        }
    };
    let fbx_light_units = light_units("fbx_light_units", from == MeshFormat::Fbx || to == MeshFormat::Fbx, "fbx")?;
    let usd_light_units = light_units(
        "usd_light_units",
        matches!(from, MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc) || matches!(to, MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc),
        "usdz, usda or usdc",
    )?;
    // `dae_morphs`: refused where the file written is not Collada.
    let dae_morphs = match value("dae_morphs") {
        None => mesh::DaeMorphs::default(),
        Some(_) if to != MeshFormat::Dae => {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "param_unsupported",
                "`dae_morphs` applies when `to` is dae".into(),
                Some("dae_morphs"),
            ))
        }
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "bake" => mesh::DaeMorphs::Bake,
            "keep" => mesh::DaeMorphs::Keep,
            _ => {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "param_invalid",
                    format!("`dae_morphs` is {v:?}; it takes bake or keep"),
                    Some("dae_morphs"),
                ))
            }
        },
    };
    // `usdz_root`: refused where the file written is not a USDZ. A USD input
    // does not make it apply: only the file written does.
    let usdz_root = match value("usdz_root") {
        None => mesh::UsdzRoot::default(),
        Some(_) if to != MeshFormat::Usdz => {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "param_unsupported",
                "`usdz_root` applies when `to` is usdz".into(),
                Some("usdz_root"),
            ))
        }
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "usda" => mesh::UsdzRoot::Usda,
            "usdc" => mesh::UsdzRoot::Usdc,
            _ => {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "param_invalid",
                    format!("`usdz_root` is {v:?}; it takes usda or usdc"),
                    Some("usdz_root"),
                ))
            }
        },
    };
    // `usd_clips`: refused where the file written is not USD.
    let usd_clips = match value("usd_clips") {
        None => None,
        Some(_) if !matches!(to, MeshFormat::Usda | MeshFormat::Usdc | MeshFormat::Usdz) => {
            return Err(reject(
                StatusCode::BAD_REQUEST,
                "param_unsupported",
                "`usd_clips` applies when `to` is usda, usdc or usdz".into(),
                Some("usd_clips"),
            ))
        }
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "all" => Some(mesh::UsdClips::All),
            "first" => Some(mesh::UsdClips::First),
            _ => {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "param_invalid",
                    format!("`usd_clips` is {v:?}; it takes all or first"),
                    Some("usd_clips"),
                ))
            }
        },
    };
    let package = match value("package") {
        None => Packaging::File,
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "file" => Packaging::File,
            "zip" => Packaging::Zip,
            _ => {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "param_invalid",
                    format!("`package` is {v:?}; it takes file or zip"),
                    Some("package"),
                ))
            }
        },
    };
    let ktx2_textures = match value("ktx2_textures") {
        None => Ktx2Textures::Png,
        Some(v) => match v.trim().to_ascii_lowercase().as_str() {
            "png" => Ktx2Textures::Png,
            "keep" => Ktx2Textures::Keep,
            _ => {
                return Err(reject(
                    StatusCode::BAD_REQUEST,
                    "param_invalid",
                    format!("`ktx2_textures` is {v:?}; it takes png or keep"),
                    Some("ktx2_textures"),
                ))
            }
        },
    };
    Ok(ConvertQuery { from, to, read_as, write_as, fbx_colors, fbx_light_units, usd_light_units, dae_morphs, usdz_root, usd_clips, package, ktx2_textures })
}

/// The bytes conversion uploads may hold in memory at once: as many uploads
/// at the input cap as there are slots, the bound the slots used to give by
/// being taken before the upload.
fn conversion_budget() -> &'static BodyBudget {
    static BUDGET: std::sync::OnceLock<BodyBudget> = std::sync::OnceLock::new();
    BUDGET.get_or_init(|| BodyBudget::new(slot_count(), mesh::MAX_INPUT_BYTES, BODY_TIMEOUT))
}

/// The upload, up to [`mesh::MAX_INPUT_BYTES`], within [`BODY_TIMEOUT`], each
/// byte charged to [`conversion_budget`] as it arrives. A raw `Body` bypasses
/// axum's DefaultBodyLimit, so this is the limit; it matches the converter's
/// own. An oversize body is 413, one that breaks off 400, one that stalls 408,
/// and one the budget has no room for 503.
async fn read_body(body: Body) -> Result<Buffered, Response> {
    conversion_budget().read(body, mesh::MAX_INPUT_BYTES).await.map_err(|e| {
        let message = match &e {
            BodyError::TimedOut(d) => format!("the upload took longer than {} s", d.as_secs()),
            other => other.message(),
        };
        let mut resp = reject(e.status(), e.code(), message, None);
        if matches!(e, BodyError::Busy) {
            resp.headers_mut().insert(header::RETRY_AFTER, axum::http::HeaderValue::from_static("1"));
        }
        resp
    })
}

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
                vendor_job_id: None,
                generation_id: None,
                status: None,
                replaced_by: None,
                retryable: None,
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

    const TRIANGLE_STL: &str =
        "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";

    fn stl_to_obj() -> Vec<(String, String)> {
        vec![("from".into(), "stl".into()), ("to".into(), "obj".into())]
    }

    /// Yields to the runtime until `done` holds. A finished or dropped body
    /// frees its slot on the task that sends it, a scheduling step later.
    async fn settles(done: impl Fn() -> bool) -> bool {
        for _ in 0..1000 {
            if done() {
                return true;
            }
            tokio::task::yield_now().await;
        }
        done()
    }

    /// A body that is read must end when its last byte is read, not at its
    /// send deadline.
    async fn within_5s<T>(read: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(std::time::Duration::from_secs(5), read).await.expect("the body ends once read")
    }

    /// The only test in this binary that takes the process-wide slots, so
    /// the counts it reads are its own.
    #[tokio::test]
    async fn a_conversion_keeps_its_slot_until_the_file_is_sent() {
        let slots = conversion_slots();
        let free = slots.available_permits();

        let resp = convert(&stl_to_obj(), Body::from(TRIANGLE_STL)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(slots.available_permits(), free - 1, "a file not yet sent still holds its slot");
        let obj = within_5s(axum::body::to_bytes(resp.into_body(), usize::MAX)).await.unwrap();
        assert!(String::from_utf8_lossy(&obj).contains("\nf "), "the file arrives whole");
        assert!(settles(|| slots.available_permits() == free).await, "a sent file frees its slot");

        let resp = convert(&stl_to_obj(), Body::from(TRIANGLE_STL)).await;
        assert_eq!(slots.available_permits(), free - 1);
        drop(resp);
        assert!(settles(|| slots.available_permits() == free).await, "a client that hangs up frees its slot");
    }

    #[tokio::test]
    async fn a_file_of_many_pieces_arrives_whole_and_then_frees_its_slot() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let file: Vec<u8> = (0..3 * SEND_CHUNK + 5).map(|i| (i % 251) as u8).collect();
        let body = slot_held_body(file.clone(), permit, std::time::Duration::from_secs(60));
        assert_eq!(slots.available_permits(), 0);
        let read = within_5s(axum::body::to_bytes(body, usize::MAX)).await.unwrap();
        assert_eq!(read.as_ref(), file.as_slice());
        assert!(settles(|| slots.available_permits() == 1).await);
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_download_loses_its_slot_and_fails_at_the_deadline() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = slots.clone().try_acquire_owned().unwrap();
        let body = slot_held_body(vec![7u8; 4 * SEND_CHUNK], permit, std::time::Duration::from_secs(5));
        // Nobody reads. With the clock paused, sleeping lets the deadline pass.
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        assert_eq!(slots.available_permits(), 1, "the deadline frees the slot of a reader that stalled");
        let read = axum::body::to_bytes(body, usize::MAX).await;
        assert!(read.is_err(), "the transfer fails rather than ending short: {read:?}");
    }

    /// N4 (P9): an upload still arriving holds no conversion slot; the slot is
    /// taken once the file is all there. A stalled upload used to hold one of
    /// the 2 to 8 slots for the whole 120 s upload deadline.
    #[tokio::test]
    async fn a_stalled_upload_holds_no_conversion_slot() {
        use futures::StreamExt;
        let glb = mesh::glb::write_scene(&mesh::fixtures::textured()).unwrap();
        let query = || -> Vec<(String, String)> { vec![("from".into(), "glb".into()), ("to".into(), "obj".into())] };
        let slots = Arc::new(Semaphore::new(1));
        let (polled_tx, polled_rx) = tokio::sync::oneshot::channel::<()>();
        let first = futures::stream::once(async move {
            let _ = polled_tx.send(());
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"glTF"))
        });
        let stalled = {
            let (slots, query) = (slots.clone(), query());
            tokio::spawn(async move { convert_in(slots, &query, Body::from_stream(first.chain(futures::stream::pending()))).await })
        };
        polled_rx.await.unwrap();
        let resp = within_5s(convert_in(slots.clone(), &query(), Body::from(glb))).await;
        stalled.abort();
        assert_eq!(resp.status(), StatusCode::OK, "a stalled upload held the only conversion slot");
    }

    /// A zip is sent like any converted file: it holds its slot until it is
    /// read, and a reader that stalls loses the slot at the send deadline and
    /// gets a failed transfer. The texture is a 256 × 256 stored PNG, so the
    /// zip (which stores images) is several send pieces long.
    #[tokio::test(start_paused = true)]
    async fn a_zip_keeps_its_slot_until_sent_and_loses_it_at_the_deadline() {
        let mut scene = mesh::fixtures::textured();
        scene.images[0].data = mesh::fixtures::encode_png(256, 256, &[200u8, 200, 200, 255].repeat(256 * 256));
        let glb = mesh::glb::write_scene(&scene).unwrap();
        let zip_query = || -> Vec<(String, String)> {
            vec![("from".into(), "glb".into()), ("to".into(), "obj".into()), ("package".into(), "zip".into())]
        };

        let slots = Arc::new(Semaphore::new(1));
        let resp = convert_in(slots.clone(), &zip_query(), Body::from(glb.clone())).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "application/zip");
        assert_eq!(slots.available_permits(), 0, "a zip not yet sent holds its slot");
        let zip = within_5s(axum::body::to_bytes(resp.into_body(), usize::MAX)).await.unwrap();
        assert!(zip.len() > 3 * SEND_CHUNK, "{} bytes", zip.len());
        assert!(mesh::zip::Archive::parse(&zip).is_ok(), "the zip arrives whole");
        assert!(settles(|| slots.available_permits() == 1).await, "a sent zip frees its slot");

        let resp = convert_in(slots.clone(), &zip_query(), Body::from(glb)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let len: usize = resp.headers()[header::CONTENT_LENGTH].to_str().unwrap().parse().unwrap();
        // Nobody reads. With the clock paused, sleeping lets the deadline pass.
        tokio::time::sleep(send_limit(len) + std::time::Duration::from_secs(1)).await;
        assert_eq!(slots.available_permits(), 1, "the deadline frees the slot of a reader that stalled");
        let read = axum::body::to_bytes(resp.into_body(), usize::MAX).await;
        assert!(read.is_err(), "the transfer fails rather than ending short");
    }

    /// Serves one file of `len` bytes through hyper on a real socket, holding
    /// `slots`' only permit, and returns the address.
    async fn serve_one_file(slots: Arc<Semaphore>, len: usize, limit: std::time::Duration) -> std::net::SocketAddr {
        let app = axum::Router::new().route(
            "/f",
            axum::routing::get(move || {
                let permit = slots.clone().try_acquire_owned().expect("one request per server");
                async move {
                    ([(header::CONTENT_LENGTH, len.to_string())], slot_held_body(vec![1u8; len], permit, limit))
                        .into_response()
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        addr
    }

    /// Reads a whole HTTP/1.1 exchange until the server closes it, and returns
    /// the declared length and the number of body bytes that arrived.
    async fn read_response(mut conn: tokio::net::TcpStream) -> (usize, usize) {
        use tokio::io::AsyncReadExt;
        let mut got = Vec::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            match conn.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
            }
        }
        let head_end = got.windows(4).position(|w| w == b"\r\n\r\n").expect("a response head") + 4;
        let head = String::from_utf8_lossy(&got[..head_end]).to_ascii_lowercase();
        let declared = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length: "))
            .and_then(|v| v.trim().parse().ok())
            .expect("a content-length");
        (declared, got.len() - head_end)
    }

    #[tokio::test]
    async fn over_a_real_connection_a_read_file_arrives_whole() {
        use tokio::io::AsyncWriteExt;
        let slots = Arc::new(Semaphore::new(1));
        let len = 3 * SEND_CHUNK + 5;
        let addr = serve_one_file(slots.clone(), len, std::time::Duration::from_secs(60)).await;
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /f HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n").await.unwrap();
        assert_eq!(within_5s(read_response(conn)).await, (len, len));
        assert!(settles(|| slots.available_permits() == 1).await);
    }

    /// The file is far larger than the socket buffers, so a client that reads
    /// nothing stalls the transfer. At the deadline its slot is free, and what
    /// it then reads is shorter than the length it was promised.
    #[tokio::test]
    async fn over_a_real_connection_a_stalled_reader_gets_a_failed_transfer() {
        use tokio::io::AsyncWriteExt;
        let slots = Arc::new(Semaphore::new(1));
        let len = 64 * 1024 * 1024;
        let addr = serve_one_file(slots.clone(), len, std::time::Duration::from_millis(500)).await;
        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /f HTTP/1.1\r\nhost: x\r\n\r\n").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        assert_eq!(slots.available_permits(), 1, "the stalled reader no longer holds the slot");
        let (declared, arrived) = within_5s(read_response(conn)).await;
        assert_eq!(declared, len);
        assert!(arrived < len, "the transfer is visibly short: {arrived} of {len}");
    }

    #[test]
    fn the_send_limit_grows_with_the_file() {
        assert_eq!(send_limit(0), BODY_TIMEOUT);
        assert_eq!(send_limit(300 * MIN_SEND_BYTES_PER_SEC), BODY_TIMEOUT + std::time::Duration::from_secs(300));
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

    fn with_ktx2_textures(value: Option<&str>) -> Vec<(String, String)> {
        let mut pairs = stl_to_obj();
        pairs.extend(value.map(|v| ("ktx2_textures".to_string(), v.to_string())));
        pairs
    }

    /// `ktx2_textures` is `png` unless it is `keep`, in either case and with
    /// spaces around it, for any pair of containers.
    #[test]
    fn ktx2_textures_takes_png_or_keep_in_any_case() {
        let parsed = |v: Option<&str>| parse_query(&with_ktx2_textures(v)).ok().map(|q| q.ktx2_textures);
        assert_eq!(parsed(None), Some(Ktx2Textures::Png));
        assert_eq!(parsed(Some("png")), Some(Ktx2Textures::Png));
        assert_eq!(parsed(Some(" PNG ")), Some(Ktx2Textures::Png));
        assert_eq!(parsed(Some("keep")), Some(Ktx2Textures::Keep));
        assert_eq!(parsed(Some(" Keep")), Some(Ktx2Textures::Keep));
        // Whatever the containers: there is no `param_unsupported` case.
        let mut glb_to_fbx = vec![("from".to_string(), "glb".to_string()), ("to".to_string(), "fbx".to_string())];
        glb_to_fbx.push(("ktx2_textures".into(), "keep".into()));
        assert!(parse_query(&glb_to_fbx).is_ok());
    }

    /// Any other word, and an empty one, is `param_invalid` naming the
    /// parameter; giving it twice is `param_repeated`.
    #[tokio::test]
    async fn ktx2_textures_refuses_other_words() {
        async fn refusal(pairs: Vec<(String, String)>) -> (StatusCode, serde_json::Value) {
            let resp = parse_query(&pairs).err().expect("refused");
            let status = resp.status();
            (status, serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap()).unwrap())
        }
        for word in ["gamma", "", "ktx2", "png,keep"] {
            let (status, body) = refusal(with_ktx2_textures(Some(word))).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{word:?}");
            assert_eq!((&body["error"]["code"], &body["error"]["param"]), (&serde_json::json!("param_invalid"), &serde_json::json!("ktx2_textures")), "{word:?}: {body}");
        }
        let mut twice = with_ktx2_textures(Some("keep"));
        twice.push(("ktx2_textures".into(), "png".into()));
        let (status, body) = refusal(twice).await;
        assert_eq!((status, &body["error"]["code"]), (StatusCode::BAD_REQUEST, &serde_json::json!("param_repeated")), "{body}");
    }

    /// A scene whose other images already fill the texture budget has no room
    /// for its KTX2, which the converter leaves out and reports as an
    /// `image_formats` loss with the budget as the reason. Built in memory:
    /// the filler is a stored PNG of exactly the budget, of which only the
    /// signature is read.
    #[test]
    fn a_ktx2_past_the_texture_budget_is_a_reported_loss() {
        use mesh::scene::{Image, ImageFormat, Texture, TextureRef};
        const BASISU: &[u8] = include_bytes!("../../providers/model3d/mock_data/textured.basisu.glb");
        let mut scene = mesh::read_scene(BASISU, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
        let mut filler = vec![0u8; mesh::normalize::TEXTURE_BUDGET];
        filler[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        scene.images.push(Image { name: Some("filler".into()), format: ImageFormat::Png, data: filler });
        scene.textures.push(Texture { name: None, image: scene.images.len() - 1, sampler: Default::default() });
        scene.materials[0].emissive_texture = Some(TextureRef { texture: scene.textures.len() - 1, uv_set: 0, transform: None });
        let glb = mesh::glb::write_scene(&scene).unwrap();
        // The KTX2 as the converter will see it, by the name it will report.
        let seen = mesh::read_scene(&glb, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
        let ktx2 = seen.images.iter().enumerate().find(|(_, i)| i.format == ImageFormat::Ktx2).expect("the KTX2 survives the glb");
        let name = ktx2.1.name.clone().unwrap_or_else(|| format!("image {}", ktx2.0));

        let query = ConvertQuery {
            from: MeshFormat::Glb,
            to: MeshFormat::Stl,
            read_as: None,
            write_as: None,
            fbx_colors: mesh::FbxColorSpace::default(),
            fbx_light_units: mesh::LightUnits::default(),
            usd_light_units: mesh::LightUnits::default(),
            dae_morphs: mesh::DaeMorphs::default(),
            usdz_root: mesh::UsdzRoot::default(),
            usd_clips: None,
            package: Packaging::File,
            ktx2_textures: Ktx2Textures::Png,
        };
        let (written, _) = convert_bytes(&glb, &query).expect("converts");
        let lost: Vec<&mesh::Loss> = written.losses.iter().filter(|l| l.kind == mesh::LossKind::ImageFormats).collect();
        assert_eq!(lost.len(), 1, "{:?}", written.losses);
        assert_eq!(lost[0].what, format!("texture {name} (KTX2)"));
        assert_eq!(lost[0].why, mesh::normalize::OVER_BUDGET);
    }

    /// The vendor-style glb of the mock `mock/basisu-3d` model (a KTX2 base
    /// colour map), converted: PNG by default, KTX2 kept only for a glb or
    /// gltf under `keep`, and a KTX2 that cannot be decoded reported with the
    /// decoder's reason.
    #[test]
    fn convert_bytes_applies_ktx2_textures_to_glb_and_gltf_only() {
        use mesh::scene::ImageFormat::{Ktx2, Png};
        const BASISU: &[u8] = include_bytes!("../../providers/model3d/mock_data/textured.basisu.glb");
        let convert = |glb: &[u8], to: MeshFormat, ktx2_textures: Ktx2Textures| {
            let query = ConvertQuery {
                from: MeshFormat::Glb,
                to,
                read_as: None,
                write_as: None,
                fbx_colors: mesh::FbxColorSpace::default(),
                fbx_light_units: mesh::LightUnits::default(),
                usd_light_units: mesh::LightUnits::default(),
                dae_morphs: mesh::DaeMorphs::default(),
                usdz_root: mesh::UsdzRoot::default(),
                usd_clips: None,
                package: Packaging::File,
                ktx2_textures,
            };
            convert_bytes(glb, &query).expect("converts")
        };
        let images = |written: &mesh::SceneWrite, to: MeshFormat| {
            let scene = mesh::read_scene(&written.package.files[0].bytes, to, &mesh::NoSidecars).unwrap().scene;
            scene.images.iter().map(|i| i.format).collect::<Vec<_>>()
        };
        for (to, ktx2_textures, want) in [
            (MeshFormat::Glb, Ktx2Textures::Png, Png),
            (MeshFormat::Glb, Ktx2Textures::Keep, Ktx2),
            (MeshFormat::Gltf, Ktx2Textures::Keep, Ktx2),
            (MeshFormat::Fbx, Ktx2Textures::Keep, Png),
            (MeshFormat::Fbx, Ktx2Textures::Png, Png),
        ] {
            let (written, _) = convert(BASISU, to, ktx2_textures);
            assert_eq!(images(&written, to), [want], "{to} {ktx2_textures:?}");
            assert!(written.losses.iter().all(|l| l.kind != mesh::LossKind::ImageFormats), "{to} {ktx2_textures:?}: {:?}", written.losses);
        }

        let mut scene = mesh::read_scene(BASISU, MeshFormat::Glb, &mesh::NoSidecars).unwrap().scene;
        let cut = scene.images[0].data.len() - 200;
        scene.images[0].data.truncate(cut);
        let broken = mesh::glb::write_scene(&scene).unwrap();
        let (written, _) = convert(&broken, MeshFormat::Fbx, Ktx2Textures::Png);
        let loss = written.losses.iter().find(|l| l.kind == mesh::LossKind::ImageFormats).expect("the lost map is reported");
        assert!(loss.why.starts_with("litegen could not decode it to PNG for this container: "), "{loss}");
        let (kept, _) = convert(&broken, MeshFormat::Glb, Ktx2Textures::Keep);
        assert!(kept.losses.iter().all(|l| l.kind != mesh::LossKind::ImageFormats), "kept as is, so nothing is lost: {:?}", kept.losses);
    }
}
