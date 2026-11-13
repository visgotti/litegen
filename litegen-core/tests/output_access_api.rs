//! Who may open stored outputs (`docs/storage-access.md`), over the real route
//! table: the asset route's checks, the URLs every answer hands out, the file
//! route's owner scope.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use base64::Engine;
use litegen::access::signing::LinkSigner;
use litegen::api::middleware::DEFAULT_APP_ID;
use litegen::db::DatabaseStore;
use litegen::types::AppFileAccessUpsert;
use std::sync::Arc;
use tower::ServiceExt;

use harness::{app_with, json_body, poll_until_terminal, submit};

fn secrets_key() -> String {
    base64::engine::general_purpose::STANDARD.encode([9u8; 32])
}

/// The full app, keyed for signing with [`secrets_key`], over a fresh asset dir.
async fn app() -> (Router, Arc<litegen::db::sqlite::SqliteDatabase>) {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    app_with(&dir, |c| c.secrets_key = Some(secrets_key())).await
}

async fn set_access(db: &litegen::db::sqlite::SqliteDatabase, app_id: &str, mode: Option<&str>, scope: Option<&str>, ttl: Option<i64>) {
    db.put_app_file_access(&AppFileAccessUpsert {
        app_id: app_id.into(),
        mode: mode.map(str::to_string),
        owner_scope: scope.map(str::to_string),
        signed_url_ttl_seconds: ttl,
        keep_outputs: None,
        retention_days: None,
        signed_links: None,
    })
    .await
    .unwrap();
}

/// The path (and query) of an absolute URL litegen minted.
fn path_of(url: &str) -> &str {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    &after_scheme[after_scheme.find('/').unwrap_or(after_scheme.len())..]
}

async fn get(app: &Router, uri: &str) -> axum::response::Response {
    app.clone().oneshot(Request::get(uri).body(Body::empty()).unwrap()).await.unwrap()
}

/// A completed mock 3D generation: its id and its glb's stored key.
async fn mesh(app: &Router) -> (String, String) {
    let id = submit(app, "mock/mesh-3d", "a teapot").await;
    let done = poll_until_terminal(app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    let url = done["assets"].as_array().unwrap().iter().find(|a| a["kind"] == "mesh").unwrap()["url"].as_str().unwrap().to_string();
    let key = path_of(&url).strip_prefix("/v1/models3d/assets/").expect("litegen's own asset route").split('?').next().unwrap().to_string();
    (id, key)
}

fn asset_uri(key: &str) -> String {
    format!("/v1/models3d/assets/{key}")
}

fn signed_uri(signer: &LinkSigner, key: &str, expires: i64) -> String {
    format!("{}?{}", asset_uri(key), signer.query(key, expires))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

async fn error_code(resp: axum::response::Response) -> String {
    json_body(resp).await["error"]["code"].as_str().unwrap_or_default().to_string()
}

// ─── The asset route ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_public_app_serves_its_files_unsigned_as_before() {
    let (app, _db) = app().await;
    let (_, key) = mesh(&app).await;
    let resp = get(&app, &asset_uri(&key)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["cache-control"], "public, max-age=31536000, immutable");
}

#[tokio::test]
async fn a_signed_app_serves_a_valid_link_and_nothing_unsigned() {
    let (app, db) = app().await;
    let (_, key) = mesh(&app).await;
    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, None).await;
    assert_eq!(get(&app, &asset_uri(&key)).await.status(), StatusCode::NOT_FOUND, "unsigned");

    let signer = LinkSigner::from_secrets_key(Some(&secrets_key()));
    let resp = get(&app, &signed_uri(&signer, &key, now() + 600)).await;
    assert_eq!(resp.status(), StatusCode::OK);
    // Cached privately, and never past the link's own expiry.
    let cc = resp.headers()["cache-control"].to_str().unwrap().to_string();
    let max_age: i64 = cc.strip_prefix("private, max-age=").expect(&cc).parse().unwrap();
    assert!((595..=600).contains(&max_age), "{cc}");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..4], b"glTF", "the stored mesh itself");
}

#[tokio::test]
async fn an_expired_link_answers_link_expired() {
    let (app, db) = app().await;
    let (_, key) = mesh(&app).await;
    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, None).await;
    let signer = LinkSigner::from_secrets_key(Some(&secrets_key()));
    let resp = get(&app, &signed_uri(&signer, &key, now() - 1)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(resp).await, "link_expired");
}

#[tokio::test]
async fn a_tampered_link_is_not_found() {
    let (app, db) = app().await;
    let (_, key) = mesh(&app).await;
    let (_, other) = mesh(&app).await;
    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, None).await;
    let signer = LinkSigner::from_secrets_key(Some(&secrets_key()));
    let expires = now() + 600;
    let sig = signer.sign(&key, expires);
    for (what, uri) in [
        ("another file's signature", format!("{}?expires={expires}&signature={sig}", asset_uri(&other))),
        ("a later expiry", format!("{}?expires={}&signature={sig}", asset_uri(&key), expires + 3600)),
        ("a changed signature", format!("{}?expires={expires}&signature={}", asset_uri(&key), sig.replace(&sig[..2], if &sig[..2] == "00" { "11" } else { "00" }))),
        ("a signature from another secrets key", signed_uri(&LinkSigner::from_secrets_key(None), &key, expires)),
        ("no expiry", format!("{}?signature={sig}", asset_uri(&key))),
        ("an expiry that is not a number", format!("{}?expires=soon&signature={sig}", asset_uri(&key))),
    ] {
        assert_eq!(get(&app, &uri).await.status(), StatusCode::NOT_FOUND, "{what}");
    }
}

#[tokio::test]
async fn a_private_app_serves_nothing_from_the_asset_route() {
    let (app, db) = app().await;
    let (_, key) = mesh(&app).await;
    set_access(&db, DEFAULT_APP_ID, Some("private"), None, None).await;
    assert_eq!(get(&app, &asset_uri(&key)).await.status(), StatusCode::NOT_FOUND, "unsigned");
    // Not even a link signed while the app was `signed`: private means only
    // the file route, which checks who is asking.
    let signer = LinkSigner::from_secrets_key(Some(&secrets_key()));
    assert_eq!(get(&app, &signed_uri(&signer, &key, now() + 600)).await.status(), StatusCode::NOT_FOUND, "signed");
}

#[tokio::test]
async fn the_gateway_default_applies_to_an_app_without_its_own() {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let (app, _db) = app_with(&dir, |c| {
        c.secrets_key = Some(secrets_key());
        c.storage_access.mode = litegen::access::AccessMode::Signed;
    })
    .await;
    let (_, key) = mesh(&app).await;
    assert_eq!(get(&app, &asset_uri(&key)).await.status(), StatusCode::NOT_FOUND, "the YAML says signed");
    let signer = LinkSigner::from_secrets_key(Some(&secrets_key()));
    assert_eq!(get(&app, &signed_uri(&signer, &key, now() + 60)).await.status(), StatusCode::OK);
}

// ─── The URLs every answer hands out ────────────────────────────────────────

const MASTER: &str = "sk-output-access-master";

/// The full app behind [`MASTER`], keyed for signing, with `tweak` applied.
async fn keyed_app(tweak: impl FnOnce(&mut litegen::config::AppConfig)) -> (Router, Arc<litegen::db::sqlite::SqliteDatabase>) {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    harness::app_with_hooks_tweaked(&dir, Some(MASTER), Vec::new(), |c| {
        c.secrets_key = Some(secrets_key());
        tweak(c);
    })
    .await
}

async fn call(app: &Router, method: &str, uri: &str, auth: Option<&str>, body: Option<serde_json::Value>) -> (StatusCode, axum::http::HeaderMap, bytes::Bytes) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = auth {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    (status, headers, axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap())
}

async fn get_json_as(app: &Router, uri: &str, auth: Option<&str>) -> (StatusCode, serde_json::Value) {
    let (s, _, b) = call(app, "GET", uri, auth, None).await;
    (s, serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null))
}

async fn post_json_as(app: &Router, uri: &str, auth: &str, body: serde_json::Value) -> serde_json::Value {
    let (s, _, b) = call(app, "POST", uri, Some(auth), Some(body)).await;
    let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null);
    assert!(s.is_success(), "{uri}: {s} {v}");
    v
}

/// A new API key's secret, made with the master key.
async fn new_key(app: &Router, name: &str, webhook_url: Option<&str>) -> String {
    let mut body = serde_json::json!({ "name": name });
    if let Some(url) = webhook_url {
        body["webhook_url"] = url.into();
    }
    post_json_as(app, "/v1/keys", MASTER, body).await["key"].as_str().unwrap().to_string()
}

/// Submit a mock 3D model as `auth` and poll it to `completed`.
async fn finished_3d(app: &Router, auth: &str) -> serde_json::Value {
    let id = post_json_as(app, "/v1/models3d/generations", auth, serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..20 {
        let (_, v) = get_json_as(app, &format!("/v1/models3d/{id}"), Some(auth)).await;
        if v["status"] == "completed" {
            return v;
        }
        assert_ne!(v["status"], "failed", "{v}");
    }
    panic!("3D job {id} never finished");
}

async fn finished_video(app: &Router, auth: &str) -> serde_json::Value {
    let id = post_json_as(app, "/v1/videos/generations", auth, serde_json::json!({ "model": "mock/video-gen", "prompt": "a wave" })).await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..20 {
        let (_, v) = get_json_as(app, &format!("/v1/videos/{id}"), Some(auth)).await;
        if v["status"] == "completed" {
            return v;
        }
        assert_ne!(v["status"], "failed", "{v}");
    }
    panic!("video {id} never finished");
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// A signed link: it carries a signature and an `expires_at` about `ttl` away.
fn assert_signed(url: &str, expires_at: &serde_json::Value, ttl: i64, what: &str) {
    assert!(url.contains("signature="), "{what}: {url}");
    let at = chrono::DateTime::parse_from_rfc3339(expires_at.as_str().unwrap_or_else(|| panic!("{what}: expires_at {expires_at}"))).unwrap();
    let left = at.timestamp() - now();
    assert!((ttl - 10..=ttl + 1).contains(&left), "{what}: expires in {left}s, not {ttl}");
}

/// Fetch an absolute URL litegen handed out, with no credentials at all.
async fn fetch_anonymously(app: &Router, url: &str) -> (StatusCode, bytes::Bytes) {
    let (s, _, b) = call(app, "GET", path_of(url), None, None).await;
    (s, b)
}

#[tokio::test]
async fn public_answers_are_unchanged() {
    let (app, _db) = keyed_app(|_| {}).await;
    let done = finished_3d(&app, MASTER).await;
    let id = done["id"].as_str().unwrap();
    for a in done["assets"].as_array().unwrap() {
        assert!(a.get("expires_at").is_none(), "{a}");
        assert!(!a["url"].as_str().unwrap().contains('?'), "the stored URL itself: {a}");
    }
    let (_, row) = get_json_as(&app, &format!("/v1/generations/{id}"), Some(MASTER)).await;
    assert_eq!(row["metadata"]["assets"], done["assets"].as_array().unwrap().iter().cloned().collect::<serde_json::Value>(), "{row}");
    let video = finished_video(&app, MASTER).await;
    assert!(video.get("video_url_expires_at").is_none(), "{video}");
    let (status, _) = fetch_anonymously(&app, video["video_url"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "public links need no credentials");
}

#[tokio::test]
async fn a_signed_app_answers_links_that_work_without_credentials() {
    let (app, db) = keyed_app(|_| {}).await;
    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, Some(600)).await;

    // The 3D status answer.
    let done = finished_3d(&app, MASTER).await;
    let id = done["id"].as_str().unwrap().to_string();
    let assets = done["assets"].as_array().unwrap().clone();
    assert!(!assets.is_empty());
    for a in &assets {
        let url = a["url"].as_str().unwrap();
        assert_signed(url, &a["expires_at"], 600, "3D status");
        let (status, bytes) = fetch_anonymously(&app, url).await;
        assert_eq!(status, StatusCode::OK, "{url}");
        if let Some(sha) = a["sha256"].as_str() {
            assert_eq!(sha256_hex(&bytes), sha, "the link serves the file its entry describes");
        }
    }

    // The generation read: its assets and its result URL.
    let (_, row) = get_json_as(&app, &format!("/v1/generations/{id}"), Some(MASTER)).await;
    for a in row["metadata"]["assets"].as_array().unwrap() {
        assert_signed(a["url"].as_str().unwrap(), &a["expires_at"], 600, "generation read");
    }
    let result_url = row["result_url"].as_str().unwrap();
    assert!(result_url.contains("signature="), "{row}");
    assert_eq!(fetch_anonymously(&app, result_url).await.0, StatusCode::OK);

    // The asset list, and the generations list.
    let (_, list) = get_json_as(&app, &format!("/v1/generations/{id}/assets"), Some(MASTER)).await;
    for a in list["data"].as_array().unwrap() {
        assert_signed(a["url"].as_str().unwrap(), &a["expires_at"], 600, "asset list");
    }
    let (_, all) = get_json_as(&app, "/v1/generations", Some(MASTER)).await;
    let listed = all["data"].as_array().expect("data").iter().find(|g| g["id"] == id.as_str()).expect("listed").clone();
    assert!(listed["result_url"].as_str().unwrap().contains("signature="), "{listed}");

    // The video status answer.
    let video = finished_video(&app, MASTER).await;
    let video_url = video["video_url"].as_str().unwrap();
    assert_signed(video_url, &video["video_url_expires_at"], 600, "video status");
    assert_eq!(fetch_anonymously(&app, video_url).await.0, StatusCode::OK);

    // An image result, read back as a generation.
    let image = post_json_as(&app, "/v1/images/generations", MASTER, serde_json::json!({ "model": "mock/image-gen", "prompt": "a fox" })).await;
    let (_, image_row) = get_json_as(&app, &format!("/v1/generations/{}", image["id"].as_str().unwrap()), Some(MASTER)).await;
    let image_url = image_row["metadata"]["assets"][0]["url"].as_str().unwrap();
    assert!(image_url.contains("signature="), "{image_row}");
    assert_eq!(fetch_anonymously(&app, image_url).await.0, StatusCode::OK);

    // What was stored is not a way around the signature.
    let stored = image_url.split('?').next().unwrap();
    assert_eq!(fetch_anonymously(&app, stored).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_private_app_answers_the_file_route() {
    let (app, db) = keyed_app(|_| {}).await;
    set_access(&db, DEFAULT_APP_ID, Some("private"), None, None).await;
    let done = finished_3d(&app, MASTER).await;
    let id = done["id"].as_str().unwrap().to_string();
    for (i, a) in done["assets"].as_array().unwrap().iter().enumerate() {
        let url = a["url"].as_str().unwrap();
        assert_eq!(url, format!("http://127.0.0.1:4000/v1/generations/{id}/assets/{i}"), "{a}");
        assert!(a.get("expires_at").is_none(), "{a}");
        let (status, _, bytes) = call(&app, "GET", path_of(url), Some(MASTER), None).await;
        assert_eq!(status, StatusCode::OK, "{url}");
        if let Some(sha) = a["sha256"].as_str() {
            assert_eq!(sha256_hex(&bytes), sha, "index {i} streams the file its entry describes");
        }
        assert_eq!(fetch_anonymously(&app, url).await.0, StatusCode::UNAUTHORIZED, "the file route wants a key");
    }
    let (_, row) = get_json_as(&app, &format!("/v1/generations/{id}"), Some(MASTER)).await;
    let glb = done["assets"].as_array().unwrap().iter().position(|a| a["kind"] == "mesh" && a["format"] == "glb").unwrap();
    assert_eq!(row["result_url"], format!("http://127.0.0.1:4000/v1/generations/{id}/assets/{glb}"), "{row}");
    let video = finished_video(&app, MASTER).await;
    assert_eq!(video["video_url"], format!("http://127.0.0.1:4000/v1/generations/{}/assets/0", video["id"].as_str().unwrap()));
}

#[tokio::test]
async fn a_key_outside_the_scope_gets_no_working_link_in_signed_mode() {
    let (app, db) = keyed_app(|_| {}).await;
    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, None).await;
    let maker = new_key(&app, "maker", None).await;
    let other = new_key(&app, "other", None).await;
    let done = finished_3d(&app, &maker).await;
    let id = done["id"].as_str().unwrap().to_string();
    assert!(done["assets"][0]["url"].as_str().unwrap().contains("signature="), "the maker gets links");

    // Another key of the same org reads the generation, as today, but every
    // URL it is given is the file route, which checks who is asking.
    let (status, row) = get_json_as(&app, &format!("/v1/generations/{id}"), Some(&other)).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    for (i, a) in row["metadata"]["assets"].as_array().unwrap().iter().enumerate() {
        assert_eq!(a["url"], format!("http://127.0.0.1:4000/v1/generations/{id}/assets/{i}"), "{a}");
        assert!(a.get("expires_at").is_none(), "{a}");
    }
    assert!(!row["result_url"].as_str().unwrap().contains("signature="), "{row}");
    // The master key is never outside the scope.
    let (_, row) = get_json_as(&app, &format!("/v1/generations/{id}"), Some(MASTER)).await;
    assert!(row["result_url"].as_str().unwrap().contains("signature="), "{row}");
}

#[tokio::test]
async fn switching_an_app_to_private_applies_to_files_made_before() {
    let (app, db) = keyed_app(|_| {}).await;
    let done = finished_3d(&app, MASTER).await;
    let id = done["id"].as_str().unwrap().to_string();
    let stored = done["assets"][0]["url"].as_str().unwrap().to_string();
    assert_eq!(fetch_anonymously(&app, &stored).await.0, StatusCode::OK, "public while the app is");

    set_access(&db, DEFAULT_APP_ID, Some("private"), None, None).await;
    let (_, again) = get_json_as(&app, &format!("/v1/models3d/{id}"), Some(MASTER)).await;
    assert_eq!(again["assets"][0]["url"], format!("http://127.0.0.1:4000/v1/generations/{id}/assets/0"), "{again}");
    assert_eq!(fetch_anonymously(&app, &stored).await.0, StatusCode::NOT_FOUND, "the old link closes with the app");
}

#[tokio::test]
async fn the_webhook_carries_links_in_the_apps_mode() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let (app, db) = keyed_app(|_| {}).await;
    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, Some(900)).await;
    let key = new_key(&app, "hooked", Some(&format!("{}/hook", server.uri()))).await;
    let done = finished_3d(&app, &key).await;

    let mut delivered = None;
    for _ in 0..50 {
        if let Some(req) = server.received_requests().await.unwrap_or_default().into_iter().next() {
            delivered = Some(req);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let payload: serde_json::Value = serde_json::from_slice(&delivered.expect("a webhook delivery").body).unwrap();
    let text = payload.to_string();
    assert!(text.contains(done["id"].as_str().unwrap()), "{payload}");
    let result_url = payload.pointer("/data/result_url").or_else(|| payload.get("result_url")).and_then(|v| v.as_str()).unwrap_or_else(|| panic!("{payload}"));
    assert!(result_url.contains("signature="), "{payload}");
    assert_eq!(fetch_anonymously(&app, result_url).await.0, StatusCode::OK);
}

// ─── The file route's owner scope ───────────────────────────────────────────

/// An API key of `app_id` in `org_id`, minted straight into the database.
async fn key_in(db: &litegen::db::sqlite::SqliteDatabase, org_id: &str, app_id: &str, name: &str) -> String {
    let kp = litegen::auth::secrets::generate_key_pair();
    db.create_api_key_scoped(org_id, app_id, &kp.public_id, name, &kp.secret_hash, &kp.prefix, None, None, "generate,read", None, None)
        .await
        .unwrap();
    kp.secret
}

async fn new_app(db: &litegen::db::sqlite::SqliteDatabase, org_id: &str, slug: &str) -> String {
    let now = chrono::Utc::now();
    let app = litegen::types::Application {
        id: uuid::Uuid::new_v4().to_string(),
        org_id: org_id.into(),
        name: slug.into(),
        slug: slug.into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_application(&app).await.unwrap();
    app.id
}

#[tokio::test]
async fn the_file_route_follows_the_owner_scope() {
    use litegen::api::middleware::DEFAULT_ORG_ID;
    let (app, db) = keyed_app(|_| {}).await;
    // Apps A and B in the master key's org; app C in another org.
    let app_b = new_app(&db, DEFAULT_ORG_ID, "b").await;
    let now = chrono::Utc::now();
    let other_org = litegen::types::Organization {
        id: uuid::Uuid::new_v4().to_string(),
        name: "other".into(),
        slug: "other".into(),
        plan: "free".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    db.create_organization(&other_org).await.unwrap();
    let app_c = new_app(&db, &other_org.id, "c").await;
    let a1 = key_in(&db, DEFAULT_ORG_ID, DEFAULT_APP_ID, "a1").await;
    let a2 = key_in(&db, DEFAULT_ORG_ID, DEFAULT_APP_ID, "a2").await;
    let b1 = key_in(&db, DEFAULT_ORG_ID, &app_b, "b1").await;
    let c1 = key_in(&db, &other_org.id, &app_c, "c1").await;

    let done = finished_3d(&app, &a1).await;
    let file = format!("/v1/generations/{}/assets/0", done["id"].as_str().unwrap());
    let status = |auth: String| {
        let (app, file) = (app.clone(), file.clone());
        async move { call(&app, "GET", &file, Some(&auth), None).await.0 }
    };

    for (scope, a2_may, b1_may) in [("key", false, false), ("app", true, false), ("org", true, true)] {
        // Every mode, private included: it is litegen's own file route.
        set_access(&db, DEFAULT_APP_ID, Some("private"), Some(scope), None).await;
        assert_eq!(status(a1.clone()).await, StatusCode::OK, "{scope}: the key that made it");
        assert_eq!(status(MASTER.into()).await, StatusCode::OK, "{scope}: the master key");
        let expect = |may: bool| if may { StatusCode::OK } else { StatusCode::FORBIDDEN };
        assert_eq!(status(a2.clone()).await, expect(a2_may), "{scope}: another key of the app");
        assert_eq!(status(b1.clone()).await, expect(b1_may), "{scope}: a key of another app");
        assert_eq!(status(c1.clone()).await, StatusCode::NOT_FOUND, "{scope}: another org never learns it exists");
    }

    set_access(&db, DEFAULT_APP_ID, Some("private"), Some("key"), None).await;
    let (s, _, body) = call(&app, "GET", &file, Some(&a2), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "not_file_owner", "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("key"), "it names the scope: {body}");
    // Under `public` the file route still checks: only the stored URL is open.
    set_access(&db, DEFAULT_APP_ID, Some("public"), Some("key"), None).await;
    assert_eq!(status(a2.clone()).await, StatusCode::FORBIDDEN, "public: the file route keeps its scope");
}

/// A finished 2 s mock sprite clip made by `auth`, which the extend models take.
async fn finished_clip(app: &Router, auth: &str) -> serde_json::Value {
    let body = serde_json::json!({ "model": "mock/sprite-video", "prompt": "a knight walks", "duration_seconds": 2 });
    let id = post_json_as(app, "/v1/videos/generations", auth, body).await["id"].as_str().unwrap().to_string();
    for _ in 0..20 {
        let (_, v) = get_json_as(app, &format!("/v1/videos/{id}"), Some(auth)).await;
        if v["status"] == "completed" {
            return v;
        }
        assert_ne!(v["status"], "failed", "{v}");
    }
    panic!("video job {id} never finished");
}

#[tokio::test]
async fn chaining_a_stored_file_follows_the_owner_scope() {
    use litegen::api::middleware::DEFAULT_ORG_ID;
    let (app, db) = keyed_app(|_| {}).await;
    let a1 = key_in(&db, DEFAULT_ORG_ID, DEFAULT_APP_ID, "a1").await;
    let a2 = key_in(&db, DEFAULT_ORG_ID, DEFAULT_APP_ID, "a2").await;
    let image = post_json_as(&app, "/v1/images/generations", &a1, serde_json::json!({ "model": "mock/image-gen", "prompt": "a hero", "response_format": "b64_json" })).await;
    let mesh = finished_3d(&app, &a1).await;
    let clip = finished_clip(&app, &a1).await;
    // Each way a job takes an earlier generation's file as its input.
    let chains = [
        ("/v1/images/generations", serde_json::json!({ "model": "mock/base64-refs-image", "prompt": "the hero, waving", "reference_images": [{ "type": "generation", "value": image["id"], "role": "init" }] })),
        ("/v1/models3d/generations", serde_json::json!({ "model": "mock/rig-3d", "source_model": { "type": "generation", "value": mesh["id"] } })),
        ("/v1/videos/generations", serde_json::json!({ "model": "mock/extend-video", "prompt": "he keeps walking", "source_video": { "type": "generation", "value": clip["id"] }, "direction": "forward", "duration_seconds": 1 })),
    ];
    // Where files are kept to their owners, a key outside the owner scope
    // can't feed one into a job; under `public` the stored URL is open anyway.
    for (mode, scope, a2_may) in [("private", "key", false), ("signed", "key", false), ("private", "app", true), ("public", "key", true)] {
        set_access(&db, DEFAULT_APP_ID, Some(mode), Some(scope), None).await;
        for (uri, body) in &chains {
            let (s, _, b) = call(&app, "POST", uri, Some(&a2), Some(body.clone())).await;
            let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or_default();
            if a2_may {
                assert!(s.is_success(), "{mode}/{scope}: another key of the app chains {uri}: {s} {v}");
            } else {
                assert_eq!((s, v["error"]["code"].as_str()), (StatusCode::NOT_FOUND, Some("source_not_found")), "{mode}/{scope}: {uri}: {v}");
            }
        }
    }
    // The key that made them always may.
    set_access(&db, DEFAULT_APP_ID, Some("private"), Some("key"), None).await;
    for (uri, body) in &chains {
        let (s, _, b) = call(&app, "POST", uri, Some(&a1), Some(body.clone())).await;
        assert!(s.is_success(), "a1 chains its own file into {uri}: {s} {}", String::from_utf8_lossy(&b));
    }
}

#[tokio::test]
async fn the_end_user_is_recorded_on_each_generation() {
    let (app, _db) = keyed_app(|_| {}).await;
    let as_alice = |uri: &'static str, body: serde_json::Value| {
        let app = app.clone();
        async move {
            let req = Request::post(uri)
                .header("authorization", format!("Bearer {MASTER}"))
                .header("x-litegen-end-user", "alice")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert!(resp.status().is_success(), "{uri}: {}", resp.status());
            json_body(resp).await["id"].as_str().unwrap().to_string()
        }
    };
    let ids = [
        as_alice("/v1/images/generations", serde_json::json!({ "model": "mock/image-gen", "prompt": "a fox" })).await,
        as_alice("/v1/videos/generations", serde_json::json!({ "model": "mock/video-gen", "prompt": "a wave" })).await,
        as_alice("/v1/models3d/generations", serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a chair" })).await,
    ];
    for id in ids {
        // The submit's row write is spawned: give it a moment to land.
        let mut row = serde_json::Value::Null;
        for _ in 0..50 {
            row = get_json_as(&app, &format!("/v1/generations/{id}"), Some(MASTER)).await.1;
            if !row["metadata"]["end_user"].is_null() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(row["metadata"]["end_user"], "alice", "{id}: {row}");
    }
}

// ─── The settings: /v1/apps/{app_id}/access and /v1/admin/settings ──────────

async fn put_json_as(app: &Router, uri: &str, auth: &str, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let (s, _, b) = call(app, "PUT", uri, Some(auth), Some(body)).await;
    (s, serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null))
}

#[tokio::test]
async fn app_access_settings_round_trip_and_show_effective_values() {
    let (app, _db) = keyed_app(|c| c.storage_access.mode = litegen::access::AccessMode::Signed).await;
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/access");
    let none = serde_json::json!({ "mode": null, "owner_scope": null, "signed_url_ttl_seconds": null, "keep_outputs": null, "retention_days": null, "signed_links": null });

    let (s, v) = get_json_as(&app, &uri, Some(MASTER)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["app_id"], DEFAULT_APP_ID);
    assert_eq!(v["overrides"], none, "{v}");
    assert_eq!(v["effective"], effective_json("signed", "key", 3600), "the YAML: {v}");
    assert!(v["updated_at"].is_null(), "{v}");

    let (s, v) = put_json_as(&app, &uri, MASTER, serde_json::json!({ "mode": "private", "signed_url_ttl_seconds": 900 })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["overrides"], serde_json::json!({ "mode": "private", "owner_scope": null, "signed_url_ttl_seconds": 900, "keep_outputs": null, "retention_days": null, "signed_links": null }), "{v}");
    assert_eq!(v["effective"], effective_json("private", "key", 900), "{v}");
    assert!(v["updated_at"].is_string(), "{v}");
    assert_eq!(get_json_as(&app, &uri, Some(MASTER)).await.1["overrides"], v["overrides"], "it was stored");

    // It is what the app's files now follow.
    let done = finished_3d(&app, MASTER).await;
    assert!(done["assets"][0]["url"].as_str().unwrap().contains("/v1/generations/"), "{done}");

    // The gateway setting sits between the app's own and the YAML.
    let (s, g) = put_json_as(&app, "/v1/admin/settings", MASTER, serde_json::json!({ "storage_access_owner_scope": "org" })).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["effective_storage_access"], effective_json("signed", "org", 3600), "{g}");
    let (_, v) = get_json_as(&app, &uri, Some(MASTER)).await;
    assert_eq!(v["effective"], effective_json("private", "org", 900), "{v}");

    // Nothing set: the row goes, and the gateway's values apply again.
    let (s, v) = put_json_as(&app, &uri, MASTER, serde_json::json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["overrides"], none, "{v}");
    assert_eq!(v["effective"], effective_json("signed", "org", 3600), "{v}");
    assert!(v["updated_at"].is_null(), "{v}");
}

/// The settings in force, with keeping, retention and links at their defaults.
fn effective_json(mode: &str, owner_scope: &str, ttl: u32) -> serde_json::Value {
    serde_json::json!({
        "mode": mode, "owner_scope": owner_scope, "signed_url_ttl_seconds": ttl,
        "keep_outputs": true, "retention_days": 0, "signed_links": "store",
    })
}

#[tokio::test]
async fn app_access_takes_keep_retention_and_signed_links() {
    let (app, _db) = keyed_app(|_| {}).await;
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/access");
    let (s, v) = put_json_as(&app, &uri, MASTER, serde_json::json!({ "keep_outputs": false, "retention_days": 30, "signed_links": "litegen" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        v["overrides"],
        serde_json::json!({ "mode": null, "owner_scope": null, "signed_url_ttl_seconds": null, "keep_outputs": false, "retention_days": 30, "signed_links": "litegen" }),
        "{v}"
    );
    assert_eq!(
        v["effective"],
        serde_json::json!({ "mode": "public", "owner_scope": "key", "signed_url_ttl_seconds": 3600, "keep_outputs": false, "retention_days": 30, "signed_links": "litegen" }),
        "{v}"
    );
    assert_eq!(get_json_as(&app, &uri, Some(MASTER)).await.1["overrides"], v["overrides"], "it was stored");

    // The gateway's values apply to an app without its own, and 0 (keep until
    // deleted) is an app value of its own, not "unset".
    let (s, g) = put_json_as(&app, "/v1/admin/settings", MASTER, serde_json::json!({ "storage_access_retention_days": 90, "storage_access_keep_outputs": false, "storage_access_signed_links": "litegen" })).await;
    assert_eq!(s, StatusCode::OK, "{g}");
    assert_eq!(g["settings"]["storage_access_retention_days"], 90, "{g}");
    assert_eq!(g["effective_storage_access"]["retention_days"], 90, "{g}");
    assert_eq!(g["effective_storage_access"]["keep_outputs"], false, "{g}");
    assert_eq!(g["effective_storage_access"]["signed_links"], "litegen", "{g}");
    let (_, v) = put_json_as(&app, &uri, MASTER, serde_json::json!({ "retention_days": 0, "keep_outputs": true })).await;
    assert_eq!(v["effective"]["retention_days"], 0, "{v}");
    assert_eq!(v["effective"]["keep_outputs"], true, "{v}");
    assert_eq!(v["effective"]["signed_links"], "litegen", "the gateway's: {v}");
}

#[tokio::test]
async fn app_access_settings_need_storage_permissions() {
    let (app, _db) = keyed_app(|_| {}).await;
    let key = new_key(&app, "not-an-admin", None).await;
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/access");
    assert_eq!(get_json_as(&app, &uri, Some(&key)).await.0, StatusCode::FORBIDDEN, "an API key is not an org member");
    assert_eq!(put_json_as(&app, &uri, &key, serde_json::json!({ "mode": "public" })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get_json_as(&app, &uri, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(get_json_as(&app, "/v1/apps/no-such-app/access", Some(MASTER)).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn app_access_settings_refuse_bad_values() {
    let (app, _db) = keyed_app(|_| {}).await;
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/access");
    for (body, code) in [
        (serde_json::json!({ "mode": "open" }), "invalid_mode"),
        (serde_json::json!({ "mode": "Private" }), "invalid_mode"),
        (serde_json::json!({ "owner_scope": "team" }), "invalid_owner_scope"),
        (serde_json::json!({ "signed_url_ttl_seconds": 59 }), "invalid_ttl"),
        (serde_json::json!({ "signed_url_ttl_seconds": 604_801 }), "invalid_ttl"),
        (serde_json::json!({ "retention_days": 36_501 }), "invalid_retention"),
        (serde_json::json!({ "retention_days": -1 }), "invalid_retention"),
        (serde_json::json!({ "signed_links": "cdn" }), "invalid_signed_links"),
        (serde_json::json!({ "signed_links": "Litegen" }), "invalid_signed_links"),
        (serde_json::json!({ "keep_outputs": "no" }), "invalid_keep_outputs"),
        (serde_json::json!({ "keep_outputs": 0 }), "invalid_keep_outputs"),
    ] {
        let (s, v) = put_json_as(&app, &uri, MASTER, body.clone()).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{body}: {v}");
        assert_eq!(v["error"]["code"], code, "{body}: {v}");
    }
    // Nothing was stored by a refused write.
    assert_eq!(get_json_as(&app, &uri, Some(MASTER)).await.1["effective"]["mode"], "public");
    let (s, v) = put_json_as(&app, "/v1/admin/settings", MASTER, serde_json::json!({ "storage_access_signed_url_ttl_seconds": 30 })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = put_json_as(&app, "/v1/admin/settings", MASTER, serde_json::json!({ "storage_access_retention_days": 36_501 })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = put_json_as(&app, "/v1/admin/settings", MASTER, serde_json::json!({ "storage_access_signed_links": "cdn" })).await;
    assert!(s.is_client_error(), "{s}: {v}");
    assert_eq!(get_json_as(&app, "/v1/admin/settings", Some(MASTER)).await.1["effective_storage_access"]["retention_days"], 0, "nothing stored");
}

#[tokio::test]
async fn changing_an_apps_access_is_audited() {
    let (app, _db) = keyed_app(|_| {}).await;
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/access");
    assert_eq!(put_json_as(&app, &uri, MASTER, serde_json::json!({ "mode": "signed" })).await.0, StatusCode::OK);
    let mut found = serde_json::Value::Null;
    for _ in 0..50 {
        let (_, audit) = get_json_as(&app, "/v1/audit", Some(MASTER)).await;
        if let Some(entry) = audit["data"].as_array().and_then(|d| d.iter().find(|e| e["action"] == "app.access.update")) {
            found = entry.clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(found["target_id"], DEFAULT_APP_ID, "{found}");
}

#[tokio::test]
async fn a_logged_requests_output_follows_the_apps_mode() {
    let (app, db) = keyed_app(|_| {}).await;
    let done = finished_3d(&app, MASTER).await;
    let id = done["id"].as_str().unwrap().to_string();
    let stored = done["assets"][0]["url"].as_str().unwrap().to_string();
    // A request log's drill-down that returned the stored URL itself (an S3
    // image store answers this way), as the dashboard's trace panel shows it.
    db.insert_request_artifact(&litegen::types::RequestArtifact {
        request_id: id.clone(),
        media_type: "image".into(),
        prompt: Some("a chair".into()),
        negative_prompt: None,
        params_json: None,
        refs_meta_json: None,
        output_kind: "url".into(),
        output_value: Some(stored.clone()),
        output_mime: Some("model/gltf-binary".into()),
        output_truncated: false,
        error_message: None,
        created_at: chrono::Utc::now(),
        org_id: Some(litegen::api::middleware::DEFAULT_ORG_ID.into()),
        app_id: Some(DEFAULT_APP_ID.into()),
    })
    .await
    .unwrap();
    let uri = format!("/v1/logs/{id}/artifact");
    assert_eq!(get_json_as(&app, &uri, Some(MASTER)).await.1["output_value"], stored.as_str(), "public: as stored");

    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, None).await;
    let signed = get_json_as(&app, &uri, Some(MASTER)).await.1["output_value"].as_str().unwrap().to_string();
    assert!(signed.contains("signature="), "{signed}");
    assert_eq!(fetch_anonymously(&app, &signed).await.0, StatusCode::OK);

    set_access(&db, DEFAULT_APP_ID, Some("private"), None, None).await;
    assert_eq!(get_json_as(&app, &uri, Some(MASTER)).await.1["output_value"], format!("http://127.0.0.1:4000/v1/generations/{id}/assets/0"));
}

#[tokio::test]
async fn a_file_whose_generation_is_gone_is_not_served() {
    // Deleting an app deletes its generations; their files stay on disk until
    // the retention sweep. Whatever the defaults say, litegen serves none.
    let (app, db) = app().await;
    let (_, key) = mesh(&app).await;
    assert_eq!(get(&app, &asset_uri(&key)).await.status(), StatusCode::OK, "public while its row exists");
    assert!(db.delete_application(DEFAULT_APP_ID).await.unwrap());
    assert_eq!(get(&app, &asset_uri(&key)).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_litegen_signed_link_opens_without_credentials() {
    let (app, _db) = keyed_app(|c| {
        c.storage_access.mode = litegen::access::AccessMode::Signed;
        c.storage_access.signed_links = litegen::access::SignedLinks::Litegen;
    })
    .await;
    let done = finished_3d(&app, MASTER).await;
    let url = done["assets"][0]["url"].as_str().unwrap().to_string();
    let id = done["id"].as_str().unwrap();
    assert!(url.contains(&format!("/v1/generations/{id}/assets/0/signed?expires=")), "litegen's own link: {url}");
    assert!(done["assets"][0]["expires_at"].is_string(), "{done}");
    let (status, headers, body) = call(&app, "GET", path_of(&url), None, None).await;
    assert_eq!(status, StatusCode::OK, "no credentials needed");
    assert_eq!(&body[..4], b"glTF");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(headers.get("content-security-policy").unwrap(), "sandbox");

    // A changed signature, or a link past its expiry, opens nothing.
    let tampered = format!("{}0", path_of(&url));
    let (status, _, body) = call(&app, "GET", &tampered, None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"], "invalid_link");
    let signer = LinkSigner::from_secrets_key(Some(&secrets_key()));
    let past = signer.file_route_query(id, 0, now() - 120, now() - 60);
    let (status, _, body) = call(&app, "GET", &format!("/v1/generations/{id}/assets/0/signed?{past}"), None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap()["error"]["code"], "link_expired");
    let (status, _, _) = call(&app, "GET", &format!("/v1/generations/{id}/assets/0/signed"), None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "no signature at all");
}

#[tokio::test]
async fn a_file_no_store_can_sign_gets_a_litegen_link_in_signed_mode() {
    // `signed_links: store` (the default) and a file whose URL no store holds:
    // a link that works without credentials, not the credentialed file route.
    let (app, db) = keyed_app(|c| c.storage_access.mode = litegen::access::AccessMode::Signed).await;
    let id = "litegen-vid-elsewhere";
    db.insert_generation(id, None, "mock/video-gen", "mock", "video", None, 0.0, Some(litegen::api::middleware::DEFAULT_ORG_ID), Some(DEFAULT_APP_ID)).await.unwrap();
    let assets = serde_json::json!({ "assets": [{ "index": 0, "kind": "video", "format": "mp4", "url": "https://bucket.example/v.mp4" }] });
    db.update_generation_if_active(id, "completed", 100, Some("https://bucket.example/v.mp4"), None, Some(chrono::Utc::now()), Some(&assets)).await.unwrap();
    let (s, v) = get_json_as(&app, &format!("/v1/generations/{id}"), Some(MASTER)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let url = v["metadata"]["assets"][0]["url"].as_str().unwrap();
    assert!(url.contains("/assets/0/signed?"), "{v}");
}

// ─── Revoking links ─────────────────────────────────────────────────────────

/// The keyed app in signed mode, with `signed_links` as given.
async fn signed_app(links: litegen::access::SignedLinks) -> (Router, Arc<litegen::db::sqlite::SqliteDatabase>) {
    keyed_app(move |c| {
        c.storage_access.mode = litegen::access::AccessMode::Signed;
        c.storage_access.signed_links = links;
    })
    .await
}

async fn status_of(app: &Router, url: &str) -> (StatusCode, serde_json::Value) {
    let (s, _, b) = call(app, "GET", path_of(url), None, None).await;
    (s, serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null))
}

async fn mesh_link(app: &Router, id: &str) -> String {
    let (_, v) = get_json_as(app, &format!("/v1/models3d/{id}"), Some(MASTER)).await;
    v["assets"][0]["url"].as_str().expect("a link").to_string()
}

async fn tick() {
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
}

#[tokio::test]
async fn revoking_an_apps_links_ends_them_at_once() {
    let (app, _db) = signed_app(litegen::access::SignedLinks::Store).await;
    let done = finished_3d(&app, MASTER).await;
    let id = done["id"].as_str().unwrap().to_string();
    let local = mesh_link(&app, &id).await;
    assert!(local.contains("/v1/models3d/assets/") && local.contains("issued="), "the local store's link, with its issue time: {local}");
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/access");
    put_json_as(&app, &uri, MASTER, serde_json::json!({ "signed_links": "litegen" })).await;
    let ours = mesh_link(&app, &id).await;
    assert!(ours.contains("/assets/0/signed?"), "{ours}");
    assert_eq!(status_of(&app, &local).await.0, StatusCode::OK);
    assert_eq!(status_of(&app, &ours).await.0, StatusCode::OK);

    tick().await;
    let revoked = post_json_as(&app, &format!("{uri}/revoke-links"), MASTER, serde_json::json!({})).await;
    let at = revoked["revoked_at"].as_str().expect("when");
    for link in [&local, &ours] {
        let (s, v) = status_of(&app, link).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{link}: {v}");
        assert_eq!(v["error"]["code"], "link_revoked", "{v}");
    }
    let (_, access) = get_json_as(&app, &uri, Some(MASTER)).await;
    assert_eq!(access["links_revoked_at"], at, "{access}");

    // A link minted after the revocation works.
    tick().await;
    assert_eq!(status_of(&app, &mesh_link(&app, &id).await).await.0, StatusCode::OK);
}

#[tokio::test]
async fn revoking_one_generations_links_spares_the_others() {
    let (app, _db) = signed_app(litegen::access::SignedLinks::Litegen).await;
    let (one, two) = (finished_3d(&app, MASTER).await, finished_3d(&app, MASTER).await);
    let (one, two) = (one["id"].as_str().unwrap().to_string(), two["id"].as_str().unwrap().to_string());
    let (link_one, link_two) = (mesh_link(&app, &one).await, mesh_link(&app, &two).await);
    tick().await;
    post_json_as(&app, &format!("/v1/generations/{one}/revoke-links"), MASTER, serde_json::json!({})).await;
    assert_eq!(status_of(&app, &link_one).await.0, StatusCode::FORBIDDEN);
    assert_eq!(status_of(&app, &link_two).await.0, StatusCode::OK, "another generation's link");
}

#[tokio::test]
async fn the_gateway_revocation_ends_every_litegen_link() {
    let (app, _db) = signed_app(litegen::access::SignedLinks::Litegen).await;
    let id = finished_3d(&app, MASTER).await["id"].as_str().unwrap().to_string();
    let link = mesh_link(&app, &id).await;
    tick().await;
    let v = post_json_as(&app, "/v1/admin/revoke-links", MASTER, serde_json::json!({})).await;
    assert!(v["revoked_at"].is_string(), "{v}");
    assert_eq!(status_of(&app, &link).await.1["error"]["code"], "link_revoked");
}

#[tokio::test]
async fn a_revocation_reaches_another_replica() {
    let dir = tempfile::tempdir().unwrap().keep();
    let file = dir.join("shared.db");
    let db = Arc::new(litegen::db::sqlite::SqliteDatabase::connect(&format!("sqlite://{}?mode=rwc", file.display())).await.unwrap());
    let tweak = |c: &mut litegen::config::AppConfig| {
        c.secrets_key = Some(secrets_key());
        c.storage_access.mode = litegen::access::AccessMode::Signed;
        c.storage_access.signed_links = litegen::access::SignedLinks::Litegen;
    };
    let a = harness::app_over(db.clone(), &dir, tweak).await;
    let b = harness::app_over(db.clone(), &dir, tweak).await;
    let id = harness::submit(&a, "mock/mesh-3d", "a cube").await;
    let done = harness::poll_until_terminal(&a, &id).await;
    let link = done["assets"][0]["url"].as_str().unwrap().to_string();
    assert_eq!(status_of(&a, &link).await.0, StatusCode::OK);
    tick().await;
    let (s, _, _) = call(&b, "POST", &format!("/v1/generations/{id}/revoke-links"), None, Some(serde_json::json!({}))).await;
    assert!(s.is_success(), "{s}");
    assert_eq!(status_of(&a, &link).await.0, StatusCode::FORBIDDEN, "revoked on b, refused on a at once");
}

#[tokio::test]
async fn a_link_without_issued_dies_at_the_first_revocation() {
    let (app, _db) = signed_app(litegen::access::SignedLinks::Store).await;
    let done = finished_3d(&app, MASTER).await;
    let url = done["assets"][0]["url"].as_str().unwrap();
    let key = path_of(url).strip_prefix("/v1/models3d/assets/").expect("litegen's own asset route").split('?').next().unwrap().to_string();
    let signer = LinkSigner::from_secrets_key(Some(&secrets_key()));
    let legacy = signed_uri(&signer, &key, now() + 600);
    assert!(!legacy.contains("issued="));
    assert_eq!(get(&app, &legacy).await.status(), StatusCode::OK, "a link minted before links carried their issue time");
    post_json_as(&app, &format!("/v1/apps/{DEFAULT_APP_ID}/access/revoke-links"), MASTER, serde_json::json!({})).await;
    assert_eq!(get(&app, &legacy).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn revoking_needs_the_right_permission() {
    let (app, _db) = signed_app(litegen::access::SignedLinks::Litegen).await;
    let maker = new_key(&app, "maker", None).await;
    let other = new_key(&app, "other", None).await;
    let id = finished_3d(&app, &maker).await["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(&app, "POST", &format!("/v1/apps/{DEFAULT_APP_ID}/access/revoke-links"), Some(&maker), Some(serde_json::json!({}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "an API key is not an org member with the storage permission");
    let (s, _, _) = call(&app, "POST", &format!("/v1/generations/{id}/revoke-links"), Some(&other), Some(serde_json::json!({}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "another key, under the default owner scope `key`");
    let (s, _, _) = call(&app, "POST", &format!("/v1/generations/{id}/revoke-links"), Some(&maker), Some(serde_json::json!({}))).await;
    assert!(s.is_success(), "the key that made it: {s}");
    let (s, _, _) = call(&app, "POST", "/v1/admin/revoke-links", Some(&maker), Some(serde_json::json!({}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "the gateway's revocation is the gateway admin's");
}

#[tokio::test]
async fn a_litegen_link_closes_when_the_app_goes_private() {
    // As the local store's links do: private means the file route only.
    let (app, _db) = signed_app(litegen::access::SignedLinks::Litegen).await;
    let id = finished_3d(&app, MASTER).await["id"].as_str().unwrap().to_string();
    let link = mesh_link(&app, &id).await;
    assert_eq!(status_of(&app, &link).await.0, StatusCode::OK);
    put_json_as(&app, &format!("/v1/apps/{DEFAULT_APP_ID}/access"), MASTER, serde_json::json!({ "mode": "private" })).await;
    assert_eq!(status_of(&app, &link).await.0, StatusCode::NOT_FOUND, "the app went private");
}

// ─── Images a request log keeps inline (security fix 5, 2026-11-05) ─────────

/// `GET /v1/logs/{id}/artifact` once the request log has been written (just
/// after the answer).
async fn artifact_when_logged(app: &Router, uri: &str, auth: &str) -> serde_json::Value {
    for _ in 0..200 {
        let (status, v) = get_json_as(app, uri, Some(auth)).await;
        if status == StatusCode::OK {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{uri} was never logged");
}

/// The request log keeps each synchronous image inline (base64, with the
/// default local image store). Where an app keeps files to the key that made
/// them (`signed`, `private`), another key of the app still reads the log, but
/// it gets the file route, which refuses it, never the image's bytes, just as
/// the file route itself holds it. `public` is unchanged.
#[tokio::test]
async fn a_logged_image_goes_out_only_inside_the_owner_scope() {
    for mode in ["public", "signed", "private"] {
        let (app, db) = keyed_app(|_| {}).await;
        set_access(&db, DEFAULT_APP_ID, Some(mode), None, None).await;
        let maker = new_key(&app, "maker", None).await;
        let other = new_key(&app, "other", None).await;
        let made = post_json_as(&app, "/v1/images/generations", &maker, serde_json::json!({ "model": "mock/image-gen", "prompt": "a fox" })).await;
        let id = made["id"].as_str().unwrap().to_string();
        let b64 = made["data"][0]["b64_json"].as_str().expect("the default store answers inline").to_string();
        let uri = format!("/v1/logs/{id}/artifact");

        let own = artifact_when_logged(&app, &uri, &maker).await;
        assert_eq!((own["output_kind"].as_str(), own["output_value"].as_str()), (Some("b64"), Some(b64.as_str())), "{mode}: the maker reads its own image");
        let (_, master) = get_json_as(&app, &uri, Some(MASTER)).await;
        assert_eq!(master["output_value"].as_str(), Some(b64.as_str()), "{mode}: the master key is never outside the scope");

        let (status, seen) = get_json_as(&app, &uri, Some(&other)).await;
        assert_eq!(status, StatusCode::OK, "{mode}: another key still reads the log: {seen}");
        if mode == "public" {
            assert_eq!(seen["output_value"].as_str(), Some(b64.as_str()), "public: as before");
        } else {
            assert!(!seen.to_string().contains(&b64), "{mode}: another key must not get the image's bytes: {seen}");
            assert_eq!(seen["output_kind"], "url", "{mode}: {seen}");
            assert_eq!(seen["output_value"], format!("http://127.0.0.1:4000/v1/generations/{id}/assets/0"), "{mode}: the file route, which refuses it");
        }
    }
}

/// A cache hit hands back an earlier generation's image. Where files are kept
/// to the key that made them, keys don't share cached images: a hit would
/// hand one key another key's image bytes.
#[tokio::test]
async fn keys_kept_to_their_own_files_share_no_cached_image() {
    let (app, db) = keyed_app(|c| c.cache.enabled = true).await;
    set_access(&db, DEFAULT_APP_ID, Some("signed"), None, None).await;
    let maker = new_key(&app, "maker", None).await;
    let other = new_key(&app, "other", None).await;
    let body = serde_json::json!({ "model": "mock/image-gen", "prompt": "one fox" });
    let cache = |h: &axum::http::HeaderMap| h.get("x-litegen-cache").and_then(|v| v.to_str().ok()).map(str::to_string);

    let (s, h, _) = call(&app, "POST", "/v1/images/generations", Some(&maker), Some(body.clone())).await;
    assert_eq!((s, cache(&h).as_deref()), (StatusCode::OK, Some("miss")));
    let (s, h, _) = call(&app, "POST", "/v1/images/generations", Some(&maker), Some(body.clone())).await;
    assert_eq!((s, cache(&h).as_deref()), (StatusCode::OK, Some("hit")), "the same key still hits");
    let (s, h, b) = call(&app, "POST", "/v1/images/generations", Some(&other), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    assert_eq!(cache(&h).as_deref(), Some("miss"), "another key gets its own generation, never the maker's bytes");
}

const PASSWORD: &str = "a-long-test-password-1";

/// A signed-in browser session: its cookie and CSRF token.
struct Session {
    cookie: String,
    csrf: String,
}

impl Session {
    async fn call(&self, app: &Router, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, axum::http::HeaderMap, bytes::Bytes) {
        let mut req = Request::builder().method(method).uri(uri).header("cookie", &self.cookie);
        if !self.csrf.is_empty() {
            req = req.header("x-csrf-token", &self.csrf);
        }
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        (status, headers, axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap())
    }
}

/// POST `body` to `uri` (a signup, or accepting an invitation) and keep the
/// session it starts.
async fn signed_in(app: &Router, uri: &str, body: serde_json::Value) -> Session {
    let (s, h, b) = call(app, "POST", uri, None, Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {}", String::from_utf8_lossy(&b));
    let cookie = h
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|c| c.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("a session cookie")
        .to_string();
    let (_, _, csrf) = Session { cookie: cookie.clone(), csrf: String::new() }.call(app, "GET", "/v1/auth/csrf", None).await;
    let csrf: serde_json::Value = serde_json::from_slice(&csrf).unwrap();
    Session { cookie, csrf: csrf["csrf_token"].as_str().expect("a csrf token").to_string() }
}

/// A Member that `owner` invited to the default org, signed in.
async fn invited_member(app: &Router, owner: &Session) -> Session {
    let (_, _, orgs) = owner.call(app, "GET", "/v1/orgs", None).await;
    let orgs: serde_json::Value = serde_json::from_slice(&orgs).unwrap();
    let org_id = orgs[0]["id"].as_str().expect("the owner's org");
    let invite = serde_json::json!({ "email": "member@example.com", "role": "member" });
    let (s, _, b) = owner.call(app, "POST", &format!("/v1/orgs/{org_id}/members"), Some(invite)).await;
    assert_eq!(s, StatusCode::OK, "invite: {}", String::from_utf8_lossy(&b));
    let invited: serde_json::Value = serde_json::from_slice(&b).unwrap();
    let token = invited["_dev_token"].as_str().expect("a dev invite token");
    signed_in(app, &format!("/v1/auth/invitations/{token}/accept"), serde_json::json!({ "password": PASSWORD })).await
}

/// A person's cache hits follow what they may read (security fix 5, review
/// round 2). Under `public`, keys share the app's cached images, but a Member
/// reads only their own generations, so a key's cached image never comes
/// back to them as a hit. The Owner, who may read every generation, still
/// gets it.
#[tokio::test]
async fn a_member_gets_no_cached_image_made_by_a_key() {
    let (app, db) = keyed_app(|c| {
        c.cache.enabled = true;
        c.dev.expose_invite_tokens = true;
    })
    .await;
    set_access(&db, DEFAULT_APP_ID, Some("public"), None, None).await;
    let key = new_key(&app, "maker", None).await;
    let owner = signed_in(&app, "/v1/auth/signup", serde_json::json!({ "email": "owner@example.com", "password": PASSWORD })).await;
    let member = invited_member(&app, &owner).await;
    let body = serde_json::json!({ "model": "mock/image-gen", "prompt": "one owl" });
    let cache = |h: &axum::http::HeaderMap| h.get("x-litegen-cache").and_then(|v| v.to_str().ok()).map(str::to_string);

    let (s, h, _) = call(&app, "POST", "/v1/images/generations", Some(&key), Some(body.clone())).await;
    assert_eq!((s, cache(&h).as_deref()), (StatusCode::OK, Some("miss")));
    let (s, h, b) = member.call(&app, "POST", "/v1/images/generations", Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&b));
    assert_eq!(cache(&h).as_deref(), Some("miss"), "a Member gets its own generation, never the key's");
    let (s, h, _) = member.call(&app, "POST", "/v1/images/generations", Some(body.clone())).await;
    assert_eq!((s, cache(&h).as_deref()), (StatusCode::OK, Some("hit")), "then hits its own");
    let (s, h, _) = owner.call(&app, "POST", "/v1/images/generations", Some(body)).await;
    assert_eq!((s, cache(&h).as_deref()), (StatusCode::OK, Some("hit")), "the Owner may read the key's image, so it hits");
}
