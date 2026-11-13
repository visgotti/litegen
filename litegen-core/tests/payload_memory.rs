//! How much memory serving and keeping large files takes (review 2026-11-11,
//! P6 and P4). A file litegen serves or keeps is streamed through a small
//! buffer, never held whole, so the heap a transfer raises stays far below
//! the file's size.
//!
//! Its own test binary, because it counts every allocation of the process
//! (a `#[global_allocator]`): the tests take `ONE_AT_A_TIME` first so no two
//! measure at once, allocate their inputs before the window opens, and read
//! bodies chunk by chunk, never whole.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt;
use litegen::api::middleware::{DEFAULT_APP_ID, DEFAULT_ORG_ID};
use litegen::db::DatabaseStore;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router + in-memory sqlite

/// The system allocator, counting the bytes it has handed out (as
/// `examples/mesh_peak.rs` does).
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grew(by: usize) {
    let now = LIVE.fetch_add(by, Relaxed) + by;
    PEAK.fetch_max(now, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grew(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Opens a measurement window: the peak becomes what is alive now.
fn reset_peak() -> usize {
    let now = LIVE.load(Relaxed);
    PEAK.store(now, Relaxed);
    now
}

/// How far the heap rose above `base` since [`reset_peak`].
fn grew_since(base: usize) -> usize {
    PEAK.load(Relaxed).saturating_sub(base)
}

/// The allocator counts every thread, so the tests measure one at a time.
/// A tokio lock: the guard is held across awaits.
static ONE_AT_A_TIME: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const MIB: usize = 1024 * 1024;
/// The file the tests move: far larger than any buffer a stream may hold.
const FILE: usize = 64 * MIB;
/// The most the heap may rise while a FILE-sized file moves through.
const BOUND: usize = 8 * MIB;

/// Write `len` bytes to `path` in 1 MiB writes from one buffer, so the test
/// never holds the file itself. The first four bytes are `glTF`.
fn write_big_file(path: &std::path::Path, len: usize) {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(path).unwrap();
    let mut chunk = vec![7u8; MIB];
    chunk[..4].copy_from_slice(b"glTF");
    f.write_all(&chunk).unwrap();
    chunk[..4].copy_from_slice(&[7u8; 4]);
    for _ in 1..len / MIB {
        f.write_all(&chunk).unwrap();
    }
}

#[tokio::test]
async fn serving_a_stored_file_holds_only_a_slice_of_it_in_memory() {
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (app, db) = harness::app_with(dir.path(), |_| {}).await;
    let id = "litegen-3d-memcheck";
    let url = format!("http://127.0.0.1:4000/v1/models3d/assets/litegen/3d/{id}/model.glb");
    db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", Some("j"), 0.0, Some(DEFAULT_ORG_ID), Some(DEFAULT_APP_ID))
        .await
        .unwrap();
    let metadata = serde_json::json!({ "assets": [{ "kind": "mesh", "format": "glb", "url": url }] });
    db.update_generation_if_active(id, "completed", 100, Some(&url), None, Some(chrono::Utc::now()), Some(&metadata))
        .await
        .unwrap();
    write_big_file(&dir.path().join(format!("litegen/3d/{id}/model.glb")), FILE);

    for route in [format!("/v1/models3d/assets/litegen/3d/{id}/model.glb"), format!("/v1/generations/{id}/assets/0")] {
        let base = reset_peak();
        let resp = app.clone().oneshot(Request::get(&route).body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{route}");
        let mut body = resp.into_body().into_data_stream();
        let mut total = 0usize;
        while let Some(chunk) = body.next().await {
            total += chunk.unwrap().len();
        }
        let grew = grew_since(base);
        eprintln!("{route}: the heap rose {} KiB while 64 MiB went through", grew / 1024);
        assert_eq!(total, FILE, "{route} served the whole file");
        assert!(
            grew < BOUND,
            "serving a 64 MiB file through {route} raised the heap by {} MiB; it must stream",
            grew / MIB
        );
    }
}

#[tokio::test]
async fn keeping_a_video_handed_over_as_bytes_makes_no_copy_of_it() {
    use litegen::proxy::storage::{ImageStorage, LocalModel3dStorage};
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store: std::sync::Arc<dyn ImageStorage> = std::sync::Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));
    let data = vec![0u8; FILE];
    let base = reset_peak();
    let kept = litegen::proxy::router::rehost_video(&store, None, "g", Some(data), None, Some("video/mp4"), true, true).await;
    let grew = grew_since(base);
    eprintln!("keeping 64 MiB handed over as bytes: the heap rose {} KiB", grew / 1024);
    let kept = kept.unwrap_or_else(|e| panic!("the video was not kept: {e}"));
    assert_eq!(kept.asset.size_bytes, Some(FILE as u64));
    assert_eq!(std::fs::metadata(dir.path().join("litegen/video/g/video.mp4")).unwrap().len(), FILE as u64);
    assert!(grew < BOUND, "keeping a 64 MiB video handed over as bytes raised the heap by {} MiB; it must not be copied", grew / MIB);
}

/// A vendor on 127.0.0.1 that answers any request with a `FILE`-byte video,
/// sent with its length from one buffer (`chunk`, repeated). Returns its link.
async fn video_host(chunk: Vec<u8>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let chunk = std::sync::Arc::new(chunk);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let chunk = chunk.clone();
            tokio::spawn(async move {
                let mut req = [0u8; 4096];
                let _ = sock.read(&mut req).await;
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: {FILE}\r\n\r\n");
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for _ in 0..FILE / chunk.len() {
                    if sock.write_all(&chunk).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    format!("http://{addr}/clip.mp4")
}

#[tokio::test]
async fn keeping_a_video_from_its_link_holds_only_a_slice_of_it_in_memory() {
    use litegen::proxy::storage::{ImageStorage, LocalModel3dStorage};
    use sha2::Digest;
    let _one = ONE_AT_A_TIME.lock().await;
    let chunk: Vec<u8> = (0..64 * 1024).map(|i| (i % 251) as u8).collect();
    let mut hasher = sha2::Sha256::new();
    for _ in 0..FILE / chunk.len() {
        hasher.update(&chunk);
    }
    let want = hex::encode(hasher.finalize());
    let url = video_host(chunk).await;
    let dir = tempfile::tempdir().unwrap();
    let store: std::sync::Arc<dyn ImageStorage> = std::sync::Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));

    let base = reset_peak();
    let kept = litegen::proxy::router::rehost_video(&store, None, "g", None, Some(&url), Some("video/mp4"), true, false).await;
    let grew = grew_since(base);
    eprintln!("keeping 64 MiB from its link: the heap rose {} KiB", grew / 1024);
    let kept = kept.unwrap_or_else(|e| panic!("the video was not kept: {e}"));
    assert_eq!(kept.asset.size_bytes, Some(FILE as u64));
    assert_eq!(kept.asset.sha256.as_deref(), Some(want.as_str()), "the hash of what the vendor sent");
    assert_eq!(std::fs::metadata(dir.path().join("litegen/video/g/video.mp4")).unwrap().len(), FILE as u64);
    assert!(grew < BOUND, "keeping a 64 MiB video from its link raised the heap by {} MiB; it must stream", grew / MIB);
}

/// Review 2026-11-11 P4, NEW 3: delivering a finished 3D job's files hands
/// their bytes to the store; it makes no copy of them.
#[tokio::test]
async fn keeping_3d_files_makes_no_copy_of_them() {
    use litegen::proxy::storage::{ImageStorage, LocalModel3dStorage};
    let _one = ONE_AT_A_TIME.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store: std::sync::Arc<dyn ImageStorage> = std::sync::Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));
    let mesh = litegen::providers::Model3dFile {
        kind: litegen::types::Model3dAssetKind::Mesh,
        format: "glb".into(),
        content_type: "model/gltf-binary".into(),
        bytes: vec![7u8; FILE],
        polycount: None,
        width: None,
        height: None,
        manifest: None,
        source_url: None,
    };
    let base = reset_peak();
    let kept = litegen::proxy::router::rehost_model3d_files(&store, None, "g", vec![mesh]).await;
    let grew = grew_since(base);
    eprintln!("keeping a 64 MiB 3D file: the heap rose {} KiB", grew / 1024);
    let kept = kept.expect("kept");
    assert_eq!(kept[0].size_bytes, Some(FILE as u64));
    assert!(grew < BOUND, "keeping a 64 MiB 3D file raised the heap by {} MiB; it must not be copied", grew / MIB);
}
