//! An app's own bucket reached through a cross-account IAM role
//! (docs/storage-access.md): litegen assumes the role with the app's external
//! ID and writes with the session it gets. STS and the bucket are wiremock.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use litegen::api::middleware::{DEFAULT_APP_ID, DEFAULT_ORG_ID};
use tower::ServiceExt;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MASTER: &str = "sk-roles-master";
const ROLE: &str = "arn:aws:iam::123456789012:role/litegen-writer";

fn secrets_key() -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [5u8; 32])
}

fn assume_role_xml(key: &str, expires: chrono::DateTime<chrono::Utc>) -> String {
    format!(
        "<AssumeRoleResponse xmlns=\"https://sts.amazonaws.com/doc/2011-06-15/\"><AssumeRoleResult><Credentials>\
         <AccessKeyId>{key}</AccessKeyId><SecretAccessKey>role-secret</SecretAccessKey>\
         <SessionToken>TOKEN-{key}</SessionToken><Expiration>{}</Expiration></Credentials>\
         <AssumedRoleUser><Arn>{ROLE}/litegen</Arn><AssumedRoleId>AROA:litegen</AssumedRoleId></AssumedRoleUser>\
         </AssumeRoleResult></AssumeRoleResponse>",
        expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    )
}

/// STS answering AssumeRole (a session good for `valid_for`) and GetCallerIdentity.
async fn sts(valid_for: chrono::Duration) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("Action=AssumeRole"))
        .respond_with(ResponseTemplate::new(200).set_body_string(assume_role_xml("ASIAROLESESSION", chrono::Utc::now() + valid_for)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("Action=GetCallerIdentity"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<GetCallerIdentityResponse><GetCallerIdentityResult><Arn>arn:aws:iam::111122223333:user/litegen</Arn>\
             <UserId>AIDA</UserId><Account>111122223333</Account></GetCallerIdentityResult></GetCallerIdentityResponse>",
        ))
        .mount(&server)
        .await;
    server
}

/// A bucket that takes every write.
async fn bucket() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
    server
}

async fn app(sts: &MockServer) -> (Router, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("asset dir").keep();
    let endpoint = sts.uri();
    let (app, _db) = harness::app_with_hooks_tweaked(&dir, Some(MASTER), Vec::new(), move |c| {
        c.secrets_key = Some(secrets_key());
        c.aws.access_key_id = Some("AKIDLITEGEN".into());
        c.aws.secret_access_key = Some("litegen-secret".into());
        c.aws.sts_endpoint = Some(endpoint);
    })
    .await;
    (app, dir)
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
    let req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {MASTER}"));
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

fn role_storage(bucket: &MockServer, extra: serde_json::Value) -> serde_json::Value {
    let mut body = serde_json::json!({
        "bucket_name": "customer-bucket",
        "region": "us-east-1",
        "endpoint_url": bucket.uri(),
        "credential_mode": "assume_role",
        "role_arn": ROLE,
    });
    if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
        b.extend(e.clone());
    }
    body
}

fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.path().is_dir() { dirs.push(e.path()) } else { found.push(e.path()) }
        }
    }
    found
}

#[tokio::test]
async fn the_get_shows_the_external_id_before_a_role_is_saved() {
    let sts = sts(chrono::Duration::hours(1)).await;
    let (app, _dir) = app(&sts).await;
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/storage");
    let (s, v) = call(&app, "GET", &uri, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["configured"], false, "{v}");
    let external = v["external_id"].as_str().expect("an external id to put in the trust policy").to_string();
    assert!(external.starts_with("litegen-") && external.len() == "litegen-".len() + 32, "{external}");
    assert_eq!(v["litegen_principal_arn"], "arn:aws:iam::111122223333:user/litegen", "the principal to trust: {v}");
    assert_eq!(call(&app, "GET", &uri, None).await.1["external_id"], external.as_str(), "stable");

    let (s, other) = call(&app, "POST", &format!("/v1/orgs/{DEFAULT_ORG_ID}/apps"), Some(serde_json::json!({ "name": "second" }))).await;
    assert!(s.is_success(), "{other}");
    let (_, v) = call(&app, "GET", &format!("/v1/apps/{}/storage", other["id"].as_str().unwrap()), None).await;
    assert_ne!(v["external_id"], external.as_str(), "each app has its own");
}

#[tokio::test]
async fn a_role_bucket_signs_with_the_assumed_credentials() {
    let sts = sts(chrono::Duration::hours(1)).await;
    let bucket = bucket().await;
    let (app, _dir) = app(&sts).await;
    let uri = format!("/v1/apps/{DEFAULT_APP_ID}/storage");
    let external = call(&app, "GET", &uri, None).await.1["external_id"].as_str().unwrap().to_string();
    let (s, v) = call(&app, "PUT", &uri, Some(role_storage(&bucket, serde_json::json!({})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!((v["credential_mode"].as_str(), v["role_arn"].as_str()), (Some("assume_role"), Some(ROLE)), "{v}");

    let assumed = sts.received_requests().await.unwrap().into_iter().find(|r| String::from_utf8_lossy(&r.body).contains("Action=AssumeRole")).expect("the role was assumed on save");
    let body = String::from_utf8_lossy(&assumed.body).to_string();
    assert!(body.contains(&format!("ExternalId={external}")), "with the app's external id: {body}");
    assert!(body.contains("RoleArn=arn%3Aaws%3Aiam%3A%3A123456789012%3Arole%2Flitegen-writer"), "{body}");
    let auth = assumed.headers.get("authorization").unwrap().to_str().unwrap().to_string();
    assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDLITEGEN/") && auth.contains("/us-east-1/sts/aws4_request"), "signed as litegen: {auth}");

    // A kept image goes to the customer's bucket, written with the role's session.
    let (s, v) = call(&app, "POST", "/v1/images/generations", Some(serde_json::json!({ "model": "mock/image-gen", "prompt": "a fox" }))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let puts = bucket.received_requests().await.unwrap();
    let put = puts.iter().find(|r| r.method.as_str() == "PUT").expect("a write to the customer's bucket");
    assert!(put.url.path().starts_with("/customer-bucket/"), "{}", put.url);
    assert_eq!(put.headers.get("x-amz-security-token").unwrap(), "TOKEN-ASIAROLESESSION");
    assert!(put.headers.get("authorization").unwrap().to_str().unwrap().contains("Credential=ASIAROLESESSION/"));
}

#[tokio::test]
async fn a_failing_role_answers_400_on_save() {
    let sts = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_string(
            "<ErrorResponse><Error><Type>Sender</Type><Code>AccessDenied</Code><Message>User: arn:aws:iam::111122223333:user/litegen is not authorized to perform: sts:AssumeRole on resource: arn:aws:iam::123456789012:role/litegen-writer</Message></Error></ErrorResponse>",
        ))
        .mount(&sts)
        .await;
    let bucket = bucket().await;
    let (app, _dir) = app(&sts).await;
    let (s, v) = call(&app, "PUT", &format!("/v1/apps/{DEFAULT_APP_ID}/storage"), Some(role_storage(&bucket, serde_json::json!({})))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(v["error"]["code"], "assume_role_failed", "{v}");
    assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains("not authorized to perform: sts:AssumeRole")), "STS's own words: {v}");
    assert_eq!(call(&app, "GET", &format!("/v1/apps/{DEFAULT_APP_ID}/storage"), None).await.1["configured"], false, "nothing saved");
}

#[tokio::test]
async fn a_bad_role_arn_is_refused() {
    let sts = sts(chrono::Duration::hours(1)).await;
    let bucket = bucket().await;
    let (app, _dir) = app(&sts).await;
    for bad in ["arn:aws:s3:::customer-bucket", "arn:aws:iam::12345:role/x", "litegen-writer", ""] {
        let (s, v) = call(&app, "PUT", &format!("/v1/apps/{DEFAULT_APP_ID}/storage"), Some(role_storage(&bucket, serde_json::json!({ "role_arn": bad })))).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{bad:?}: {v}");
        assert_eq!(v["error"]["code"], "invalid_role_arn", "{bad:?}: {v}");
    }
}

async fn assume_role_calls(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().iter().filter(|r| String::from_utf8_lossy(&r.body).contains("Action=AssumeRole")).count()
}

#[tokio::test]
async fn cached_credentials_are_refreshed_before_they_expire() {
    let cfg = |sts: &MockServer| litegen::config::AwsConfig {
        access_key_id: Some("AKIDLITEGEN".into()),
        secret_access_key: Some("litegen-secret".into()),
        sts_endpoint: Some(sts.uri()),
        ..Default::default()
    };
    // A session about to end is assumed again on its next use.
    let short = sts(chrono::Duration::minutes(4)).await;
    for _ in 0..2 {
        litegen::proxy::aws_sts::assumed_credentials(&cfg(&short), "arn:aws:iam::123456789012:role/short", "litegen-x", "litegen-test").await.unwrap();
    }
    assert_eq!(assume_role_calls(&short).await, 2, "four minutes left is too little");
    // A fresh one is reused.
    let long = sts(chrono::Duration::hours(1)).await;
    for _ in 0..2 {
        litegen::proxy::aws_sts::assumed_credentials(&cfg(&long), "arn:aws:iam::123456789012:role/long", "litegen-x", "litegen-test").await.unwrap();
    }
    assert_eq!(assume_role_calls(&long).await, 1, "an hour left: cached");
}

#[tokio::test]
async fn a_presign_longer_than_the_session_becomes_a_litegen_link() {
    let sts = sts(chrono::Duration::hours(1)).await;
    let bucket = bucket().await;
    let (app, _dir) = app(&sts).await;
    let (s, v) = call(&app, "PUT", &format!("/v1/apps/{DEFAULT_APP_ID}/storage"), Some(role_storage(&bucket, serde_json::json!({})))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let access = format!("/v1/apps/{DEFAULT_APP_ID}/access");
    let mesh = |app: Router| async move {
        let (_, v) = call(&app, "POST", "/v1/models3d/generations", Some(serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a cube" }))).await;
        let id = v["id"].as_str().unwrap().to_string();
        for _ in 0..20 {
            let (_, v) = call(&app, "GET", &format!("/v1/models3d/{id}"), None).await;
            if v["status"] == "completed" {
                return v["assets"][0]["url"].as_str().unwrap().to_string();
            }
        }
        panic!("never finished");
    };
    call(&app, "PUT", &access, Some(serde_json::json!({ "mode": "signed", "signed_url_ttl_seconds": 1800 }))).await;
    let url = mesh(app.clone()).await;
    assert!(url.contains("X-Amz-Signature") && url.contains("X-Amz-Security-Token"), "within the session: the bucket's own link: {url}");
    call(&app, "PUT", &access, Some(serde_json::json!({ "mode": "signed", "signed_url_ttl_seconds": 7200 }))).await;
    let url = mesh(app.clone()).await;
    assert!(url.contains("/assets/0/signed?"), "past the session's end: litegen's link: {url}");
}

#[tokio::test]
async fn without_fallback_a_broken_bucket_keeps_nothing_in_litegens_store() {
    // A session too short to cache, so each use assumes the role again.
    let sts = sts(chrono::Duration::minutes(4)).await;
    let bucket = bucket().await;
    let (app, dir) = app(&sts).await;
    let (s, v) = call(&app, "PUT", &format!("/v1/apps/{DEFAULT_APP_ID}/storage"), Some(role_storage(&bucket, serde_json::json!({ "fallback_to_litegen_store": false })))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["fallback_to_litegen_store"], false, "{v}");
    // The role stops working.
    sts.reset().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(403).set_body_string("<ErrorResponse><Error><Code>AccessDenied</Code><Message>revoked</Message></Error></ErrorResponse>")).mount(&sts).await;
    let before = files_under(&dir);

    let (s, v) = call(&app, "POST", "/v1/images/generations", Some(serde_json::json!({ "model": "mock/image-gen", "prompt": "a fox" }))).await;
    assert_eq!(s, StatusCode::OK, "the caller still gets the image: {v}");
    assert!(v["data"][0]["b64_json"].is_string(), "{v}");
    let (s, v) = call(&app, "POST", "/v1/videos/generations", Some(serde_json::json!({ "model": "mock/video-gen", "prompt": "a wave" }))).await;
    assert!(s.is_success(), "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    for _ in 0..10 {
        let (_, v) = call(&app, "GET", &format!("/v1/videos/{id}"), None).await;
        assert_ne!(v["status"], "completed", "nowhere to keep it, so it waits: {v}");
    }
    assert_eq!(files_under(&dir), before, "nothing went to litegen's store");
}
