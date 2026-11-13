//! PixelGenie L2: the job row exists when the submit returns, it records the
//! request (never reference bytes), and a submit whose row could not be
//! written fails instead of answering with an id nothing can poll.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

mod harness; // see harness/mod.rs — real create_router over a sqlite DB

async fn post(app: &axum::Router, path: &str, body: &Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// An app over a file-backed DB, and a second connection to the same file.
async fn app_on_file() -> (axum::Router, Arc<litegen::db::sqlite::SqliteDatabase>, sqlx::SqlitePool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/litegen.db?mode=rwc", dir.path().display());
    let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&url).await.unwrap());
    let app = harness::app_over(db.clone(), dir.path(), |_| {}).await;
    let raw = sqlx::SqlitePool::connect(&url).await.unwrap();
    (app, db, raw, dir)
}

async fn refuse_generation_inserts(raw: &sqlx::SqlitePool) {
    sqlx::query("CREATE TRIGGER refuse_generations BEFORE INSERT ON generations BEGIN SELECT RAISE(ABORT, 'disk full'); END;")
        .execute(raw)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_video_submit_whose_row_cannot_be_written_fails() {
    let (app, _db, raw, _dir) = app_on_file().await;
    refuse_generation_inserts(&raw).await;
    let (status, body) = post(&app, "/v1/videos/generations", &json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["code"], "generation_record_failed", "{body}");
}

/// The caller was told the submit failed, so it is neither billed nor left
/// running: its log row says failed, and nothing tracks the job any more.
#[tokio::test]
async fn an_unrecorded_video_job_is_logged_failed_and_let_go() {
    use litegen::db::DatabaseStore;
    let (app, db, raw, _dir) = app_on_file().await;
    refuse_generation_inserts(&raw).await;
    let (status, body) = post(&app, "/v1/videos/generations", &json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let logs = db.get_request_logs(1, 10).await.unwrap().0;
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(logs[0].status, litegen::types::GenerationStatus::Failed, "{logs:?}");
    let resp = app.clone().oneshot(Request::get(format!("/v1/videos/{}", logs[0].id)).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "the router let the job go");
}

#[tokio::test]
async fn an_unrecorded_3d_job_is_logged_failed() {
    use litegen::db::DatabaseStore;
    let (app, db, raw, _dir) = app_on_file().await;
    refuse_generation_inserts(&raw).await;
    let (status, body) = post(&app, "/v1/models3d/generations", &json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let logs = db.get_request_logs(1, 10).await.unwrap().0;
    assert_eq!(logs.len(), 1, "{logs:?}");
    assert_eq!(logs[0].status, litegen::types::GenerationStatus::Failed, "{logs:?}");
}

#[tokio::test]
async fn a_3d_submit_whose_row_cannot_be_written_fails() {
    let (app, _db, raw, _dir) = app_on_file().await;
    refuse_generation_inserts(&raw).await;
    let (status, body) = post(&app, "/v1/models3d/generations", &json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["error"]["code"], "generation_record_failed", "{body}");
}

/// SEC-DBERR: the caller of an unrecorded job is told which job and that it
/// failed, never the database's own error text (tables, constraints, a
/// trigger's message). The server log has it.
#[tokio::test]
async fn an_unrecorded_job_does_not_show_the_database_error() {
    let (app, _db, raw, _dir) = app_on_file().await;
    refuse_generation_inserts(&raw).await;
    for (path, model) in [("/v1/videos/generations", "mock/video-gen"), ("/v1/models3d/generations", "mock/mesh-3d")] {
        let (status, body) = post(&app, path, &json!({ "model": model, "prompt": "a chair" })).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert_eq!(body["error"]["code"], "generation_record_failed", "{body}");
        assert!(body["error"].get("vendor_job_id").is_some(), "{body}");
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(!message.contains("disk full") && !message.contains("database"), "{model}: {message}");
    }
}

/// SEC-DBERR: a read the database fails answers a fixed 500, not the
/// database's text ("no such table: generations").
#[tokio::test]
async fn a_generations_read_error_is_not_shown() {
    let (app, _db, raw, _dir) = app_on_file().await;
    sqlx::query("ALTER TABLE generations RENAME TO generations_away").execute(&raw).await.unwrap();
    for uri in ["/v1/generations/x", "/v1/generations"] {
        let resp = app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = resp.status();
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}: {body}");
        assert_eq!(body["error"]["code"], "internal_error", "{uri}: {body}");
        assert_eq!(body["error"]["message"], "internal server error", "{uri}: {body}");
    }
}

#[tokio::test]
async fn a_video_id_answers_on_the_very_next_request() {
    let (app, _db) = harness::app_with_mock_3d().await;
    let (_, submitted) = post(&app, "/v1/videos/generations", &json!({ "model": "mock/video-gen", "prompt": "a wave" })).await;
    let id = submitted["id"].as_str().expect("an id");
    let resp = app.clone().oneshot(Request::get(format!("/v1/generations/{id}")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_row_records_the_request_but_never_reference_bytes() {
    use litegen::db::DatabaseStore;
    let (app, db) = harness::app_with_mock_3d().await;
    let png = litegen::providers::image::mock::MockProvider::placeholder_png();
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let (status, submitted) = post(&app, "/v1/models3d/generations", &json!({
        "model": "mock/mesh-3d",
        "prompt": "a red chair",
        "target_polycount": 5000,
        "reference_images": [{ "type": "base64", "value": b64, "role": "init" }],
    })).await;
    assert_eq!(status, StatusCode::OK, "{submitted}");
    let id = submitted["id"].as_str().unwrap();
    let row = db.get_generation(id).await.unwrap().expect("row written before the response");
    let req = &row.metadata.expect("metadata")["request"];
    assert_eq!(req["prompt"], "a red chair", "{req}");
    assert_eq!(req["params"]["target_polycount"], 5000, "{req}");
    let r0 = &req["reference_images"][0];
    assert_eq!(r0["role"], "init", "{req}");
    assert_eq!(r0["type"], "base64", "{req}");
    assert_eq!(r0["bytes"], png.len(), "{req}");
    use sha2::Digest;
    assert_eq!(r0["sha256"], hex::encode(sha2::Sha256::digest(&png)), "{req}");
    assert!(!req.to_string().contains(&b64), "the image itself is never stored");
}

/// `method path` with `headers`: the status, the headers and the JSON body.
async fn send(app: &axum::Router, method: &str, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// SEC-INMEM (review 2026-11-11): deleting an org lets go of the jobs this
/// replica tracks for it. Their rows went with the org, so nothing would ever
/// read or finish them; they used to stay in memory, readable by anyone who
/// knew the id.
#[tokio::test]
async fn deleting_an_org_lets_go_of_its_tracked_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_in(db, dir.path(), |_| {}, Some("test-master-key"), litegen::config::Mode::Hosted).await;
    let (status, headers, body) = send(&app, "POST", "/v1/auth/signup", Some(json!({ "email": "gone@example.com", "password": "a-long-test-password-1", "org_name": "Gone" })), &[]).await;
    assert_eq!(status, StatusCode::OK, "signup: {body}");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("session cookie")
        .to_string();
    let (_, _, csrf) = send(&app, "GET", "/v1/auth/csrf", None, &[("cookie", &cookie)]).await;
    let csrf = csrf["csrf_token"].as_str().expect("csrf").to_string();
    let session = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let (_, _, orgs) = send(&app, "GET", "/v1/orgs", None, &session).await;
    let org = orgs[0]["id"].as_str().expect("org id").to_string();
    let (status, _, key) = send(&app, "POST", "/v1/keys", Some(json!({ "name": "k", "scopes": "generate,read" })), &session).await;
    assert_eq!(status, StatusCode::CREATED, "{key}");
    let bearer = format!("Bearer {}", key["key"].as_str().expect("secret"));
    let job = |path: &'static str, model: &'static str| {
        let (app, bearer) = (app.clone(), bearer.clone());
        async move {
            let (status, _, body) = send(&app, "POST", path, Some(json!({ "model": model, "prompt": "a wave" })), &[("authorization", &bearer)]).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            body["id"].as_str().expect("job id").to_string()
        }
    };
    let video = job("/v1/videos/generations", "mock/held-video").await;
    let mesh = job("/v1/models3d/generations", "mock/mesh-3d").await;
    assert!(state.router.get_provider_job_id(&video).await.is_some(), "tracked before the delete");
    assert!(state.router.get_model3d_provider_job_id(&mesh).await.is_some(), "tracked before the delete");

    let (status, _, body) = send(&app, "DELETE", &format!("/v1/orgs/{org}"), None, &session).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(state.router.get_provider_job_id(&video).await.is_none(), "a deleted org's video job is still tracked");
    assert!(state.router.get_model3d_provider_job_id(&mesh).await.is_none(), "a deleted org's 3D job is still tracked");
}

/// A new hosted account and its org, signed in: the session cookie and its
/// CSRF token.
async fn signed_up(app: &axum::Router, email: &str, org_name: &str) -> (String, String) {
    let (status, headers, body) = send(app, "POST", "/v1/auth/signup", Some(json!({ "email": email, "password": "a-long-test-password-1", "org_name": org_name })), &[]).await;
    assert_eq!(status, StatusCode::OK, "signup: {body}");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("session cookie")
        .to_string();
    let (_, _, csrf) = send(app, "GET", "/v1/auth/csrf", None, &[("cookie", &cookie)]).await;
    (cookie, csrf["csrf_token"].as_str().expect("csrf").to_string())
}

/// SEC-INMEM, on a replica that did not handle the delete: another replica's
/// `DELETE /v1/orgs/{id}` took the org's rows, and this one still tracks its
/// running jobs. With no row there is no tenant to check, so the first read
/// of such a job, here another org's, answers 404 and lets the job go,
/// instead of polling it and handing it to whoever asked.
#[tokio::test]
async fn a_job_whose_org_another_replica_deleted_answers_no_one_here() {
    use litegen::db::DatabaseStore;
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_in(db.clone(), dir.path(), |_| {}, Some("test-master-key"), litegen::config::Mode::Hosted).await;
    let (cookie, csrf) = signed_up(&app, "gone-elsewhere@example.com", "Gone elsewhere").await;
    let session = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let (_, _, orgs) = send(&app, "GET", "/v1/orgs", None, &session).await;
    let org = orgs[0]["id"].as_str().expect("org id").to_string();
    let (status, _, key) = send(&app, "POST", "/v1/keys", Some(json!({ "name": "k", "scopes": "generate,read" })), &session).await;
    assert_eq!(status, StatusCode::CREATED, "{key}");
    let bearer = format!("Bearer {}", key["key"].as_str().expect("secret"));
    let mut jobs = Vec::new();
    for (path, model) in [("/v1/videos/generations", "mock/held-video"), ("/v1/models3d/generations", "mock/mesh-3d")] {
        let (status, _, body) = send(&app, "POST", path, Some(json!({ "model": model, "prompt": "a wave" })), &[("authorization", &bearer)]).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        jobs.push(body["id"].as_str().expect("job id").to_string());
    }
    let (video, mesh) = (&jobs[0], &jobs[1]);

    // Another replica's delete: the rows go, and nothing tells this one.
    assert!(db.delete_organization(&org).await.unwrap(), "the org is deleted");
    assert!(state.router.get_provider_job_id(video).await.is_some(), "this replica still tracks the video job");
    assert!(state.router.get_model3d_provider_job_id(mesh).await.is_some(), "this replica still tracks the 3D job");

    let (cookie, csrf) = signed_up(&app, "another@example.com", "Another").await;
    let other = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    for path in [format!("/v1/videos/{video}"), format!("/v1/models3d/{mesh}")] {
        let (status, _, body) = send(&app, "GET", &path, None, &other).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "another org reads a deleted org's job at {path}: {body}");
    }
    assert!(state.router.get_provider_job_id(video).await.is_none(), "the video job is still tracked after its read");
    assert!(state.router.get_model3d_provider_job_id(mesh).await.is_none(), "the 3D job is still tracked after its read");
}

/// SEC-INMEM, a job another replica tracks (here: one this replica let go):
/// deleting its org still stops it at its vendor, on the org's own key. The
/// org's credentials go with the org, so the key is read before the delete.
#[tokio::test]
async fn a_deleted_orgs_untracked_job_is_stopped_on_the_orgs_own_key() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let runway = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/text_to_video"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "task-1" })))
        .mount(&runway)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/tasks/task-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "task-1", "status": "RUNNING" })))
        .mount(&runway)
        .await;
    Mock::given(method("DELETE")).and(path("/v1/tasks/task-1")).respond_with(ResponseTemplate::new(204)).mount(&runway).await;
    let api_base = format!("{}/v1", runway.uri());
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_in(
        db,
        dir.path(),
        |c| {
            c.secrets_key = Some(base64::engine::general_purpose::STANDARD.encode([7u8; 32]));
            c.providers.insert(
                "runway".into(),
                serde_json::from_value(json!({ "enabled": true, "api_key": "PLATFORM_RUNWAY_KEY", "api_base": api_base })).unwrap(),
            );
        },
        Some("test-master-key"),
        litegen::config::Mode::Hosted,
    )
    .await;
    let (status, headers, body) = send(&app, "POST", "/v1/auth/signup", Some(json!({ "email": "away@example.com", "password": "a-long-test-password-1", "org_name": "Away" })), &[]).await;
    assert_eq!(status, StatusCode::OK, "signup: {body}");
    let cookie = headers
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok()?.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("session cookie")
        .to_string();
    let (_, _, csrf) = send(&app, "GET", "/v1/auth/csrf", None, &[("cookie", &cookie)]).await;
    let csrf = csrf["csrf_token"].as_str().expect("csrf").to_string();
    let session = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let (_, _, orgs) = send(&app, "GET", "/v1/orgs", None, &session).await;
    let org = orgs[0]["id"].as_str().expect("org id").to_string();
    let (status, _, stored) = send(
        &app,
        "POST",
        &format!("/v1/orgs/{org}/provider-credentials"),
        Some(json!({ "provider": "runway", "credentials": { "api_key": "ORG_RUNWAY_KEY" } })),
        &session,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "store the org's runway key: {stored}");
    let (status, _, key) = send(&app, "POST", "/v1/keys", Some(json!({ "name": "k", "scopes": "generate,read" })), &session).await;
    assert_eq!(status, StatusCode::CREATED, "{key}");
    let bearer = format!("Bearer {}", key["key"].as_str().expect("secret"));
    let (status, _, made) = send(&app, "POST", "/v1/videos/generations", Some(json!({ "model": "runway/veo3.1", "prompt": "a wave" })), &[("authorization", &bearer)]).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let id = made["id"].as_str().expect("job id").to_string();
    assert!(state.router.take_video_job(&id).await.is_some(), "tracked here until another replica's is simulated");

    let (status, _, body) = send(&app, "DELETE", &format!("/v1/orgs/{org}"), None, &session).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    // Asked in the background: 30 s at most, done as soon as it arrives.
    let mut asked = Vec::new();
    for _ in 0..300 {
        asked = runway
            .received_requests()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.method.as_str() == "DELETE" && r.url.path() == "/v1/tasks/task-1")
            .map(|r| r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        if !asked.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!asked.is_empty(), "the vendor was never asked to stop a deleted org's job");
    assert_eq!(asked, ["Bearer ORG_RUNWAY_KEY"], "asked on another key than the one that submitted it");
}
