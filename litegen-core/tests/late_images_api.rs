//! Late and asynchronous images (docs/async-images.md): an image job the
//! vendor took is delivered even when it outlasts litegen's deadline, and a
//! caller can ask not to wait for it (`Prefer: respond-async`, `wait=N`).
//! The mock runs a job for prompts with `job_ms=<n>`.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

mod harness;

const MODEL: &str = "mock/priced-image";
const PRICE: f64 = 0.10;

async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn image(prompt: &str, prefer: Option<&str>) -> Request<Body> {
    let mut req = Request::post("/v1/images/generations").header("content-type", "application/json");
    if let Some(p) = prefer {
        req = req.header("prefer", p);
    }
    req.body(Body::from(json!({ "model": MODEL, "prompt": prompt }).to_string())).unwrap()
}

async fn generation(app: &axum::Router, id: &str) -> Value {
    let (status, _, body) = call(app, Request::get(format!("/v1/generations/{id}")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// One tick of the background poller.
async fn tick(state: &Arc<litegen::api::middleware::AppState>) {
    litegen::proxy::poller::poll_once_presenting(
        &state.db,
        &state.router.registry,
        state.secrets_key,
        state.mode,
        &state.router.model3d_store,
        None,
    )
    .await;
}

async fn app() -> (axum::Router, Arc<litegen::api::middleware::AppState>) {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = Arc::new(litegen::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    harness::app_and_state_over(db, &dir).await
}

#[tokio::test]
async fn respond_async_answers_202_and_the_poller_delivers_the_image() {
    let (app, state) = app().await;
    let (status, headers, body) = call(&app, image("a lighthouse job_ms=150", Some("respond-async"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!((body["object"].as_str(), body["status"].as_str(), body["model"].as_str()), (Some("image.generation"), Some("processing"), Some(MODEL)), "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(headers.get("location").unwrap(), &format!("/v1/generations/{id}"));
    assert_eq!(headers.get("preference-applied").unwrap(), "respond-async");

    let row = generation(&app, &id).await;
    assert!(matches!(row["status"].as_str(), Some("pending" | "processing")), "{row}");
    assert!((row["cost_usd"].as_f64().unwrap() - PRICE).abs() < 1e-9, "charged when the vendor took it: {row}");

    // Still running: the poller leaves it running.
    tick(&state).await;
    assert!(matches!(generation(&app, &id).await["status"].as_str(), Some("pending" | "processing")));

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    tick(&state).await;
    let done = generation(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    let asset = &done["metadata"]["assets"][0];
    assert_eq!((asset["kind"].as_str(), asset["format"].as_str()), (Some("image"), Some("png")), "{done}");
    assert!(asset["url"].as_str().is_some_and(|u| !u.is_empty()), "{done}");
    assert!((done["cost_usd"].as_f64().unwrap() - PRICE).abs() < 1e-9, "billed once: {done}");
}

#[tokio::test]
async fn a_sync_call_that_outlasts_the_vendor_timeout_is_delivered_later() {
    let dir = tempfile::tempdir().unwrap().keep();
    let db = Arc::new(litegen::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over_with(db, &dir, |cfg| cfg.server.vendor_timeout_secs = Some(5)).await;
    let (status, _, body) = call(&app, image("a slow lighthouse job_ms=5600", None)).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
    let e = &body["error"];
    assert_eq!((e["code"].as_str(), e["status"].as_str(), e["retryable"].as_bool()), (Some("vendor_timeout"), Some("processing"), Some(false)), "{body}");
    let id = e["generation_id"].as_str().expect("the 504 names the generation").to_string();
    assert!(e["vendor_job_id"].as_str().is_some_and(|j| j.starts_with("mock-image-job-")), "{body}");

    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    tick(&state).await;
    let done = generation(&app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["metadata"]["assets"][0]["kind"], "image", "{done}");
    assert_eq!(done["result_url"], done["metadata"]["assets"][0]["url"], "{done}");
}

#[tokio::test]
async fn a_late_image_the_vendor_fails_ends_failed_with_its_reason() {
    let (app, state) = app().await;
    let (status, _, body) = call(&app, image("a lighthouse job_ms=50 job_fails", Some("respond-async"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    tick(&state).await;
    let row = generation(&app, &id).await;
    assert_eq!(row["status"], "failed", "{row}");
    assert_eq!(row["cost_usd"].as_f64(), Some(0.0), "refunded: {row}");
    assert!(row["error_message"].as_str().is_some_and(|m| m.contains("mock job failed")) || row["error"].to_string().contains("mock job failed"), "{row}");
}

#[tokio::test]
async fn wait_answers_with_the_image_when_the_job_beats_it_and_202_when_it_does_not() {
    let (app, _state) = app().await;
    let (status, headers, body) = call(&app, image("a quick lighthouse job_ms=100", Some("wait=2"))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["data"][0]["url"].is_string() || body["data"][0]["b64_json"].is_string(), "{body}");
    assert!(headers.get("preference-applied").is_none(), "nothing deferred: {headers:?}");

    let started = std::time::Instant::now();
    let (status, headers, body) = call(&app, image("a slower lighthouse job_ms=4000", Some("wait=1"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(headers.get("preference-applied").unwrap(), "wait=1");
    assert!(started.elapsed() < std::time::Duration::from_secs(3), "answered after about a second: {:?}", started.elapsed());
}

#[tokio::test]
async fn a_caller_that_keeps_nothing_is_answered_synchronously() {
    let (app, _state) = app().await;
    let mut req = image("a private lighthouse job_ms=100", Some("respond-async"));
    req.headers_mut().insert("x-litegen-keep-outputs", "false".parse().unwrap());
    let (status, headers, body) = call(&app, req).await;
    assert_eq!(status, StatusCode::OK, "the preference can't be honoured, so it is not: {body}");
    assert!(body["data"][0]["b64_json"].is_string(), "{body}");
    assert!(headers.get("preference-applied").is_none());
}

#[tokio::test]
async fn a_late_image_keeps_its_request_for_the_logs_trace() {
    let (app, _state) = app().await;
    let (status, _, body) = call(&app, image("a traced lighthouse job_ms=150", Some("respond-async"))).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let (status, _, artifact) = call(&app, Request::get(format!("/v1/logs/{id}/artifact")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{artifact}");
    assert_eq!(artifact["prompt"], "a traced lighthouse job_ms=150", "{artifact}");
    assert_eq!(artifact["media_type"], "image", "{artifact}");
}

// ─── On a model route (late-route, 2026-11-14) ──────────────────────────────
// A route may send a model to another vendor's deployment. A late job is that
// deployment's: litegen records it under that vendor and the poller asks that
// vendor about it. Asked at the model's own vendor, which never had the job,
// it was failed and refunded while the deployment's vendor ran and billed it.

/// The app with the model's own vendor `own` served (its API a stand-in that
/// knows no job: 404 to everything) and a route sending `model` to the mock
/// deployment alone. litegen waits 0 s for that deployment, so a job the mock
/// takes is late as soon as it is submitted.
async fn routed_app(own: &str, model: &str) -> (axum::Router, Arc<litegen::api::middleware::AppState>, wiremock::MockServer) {
    let vendor = wiremock::MockServer::start().await;
    let own_config = json!({ "enabled": true, "api_key": "PLATFORM_KEY", "api_base": vendor.uri() });
    let dir = tempfile::tempdir().unwrap().keep();
    let db = Arc::new(litegen::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over_with(db, &dir, |cfg| {
        cfg.providers.insert(own.to_string(), serde_json::from_value(own_config).unwrap());
        cfg.model_routes = vec![litegen::config::ModelRouteConfig {
            model: model.into(),
            deployments: vec![litegen::config::DeploymentConfig { provider: "mock".into(), weight: 1, max_retries: 0, timeout_seconds: 0, rpm_limit: 0 }],
            strategy: None,
            cache: None,
        }];
    })
    .await;
    (app, state, vendor)
}

fn routed_image(model: &str, prompt: &str) -> Request<Body> {
    Request::post("/v1/images/generations")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "model": model, "prompt": prompt }).to_string()))
        .unwrap()
}

/// The generation once the poller has ended it, ticking for up to about five seconds.
async fn until_terminal(app: &axum::Router, state: &Arc<litegen::api::middleware::AppState>, id: &str) -> Value {
    for _ in 0..100 {
        tick(state).await;
        let row = generation(app, id).await;
        if matches!(row["status"].as_str(), Some("completed" | "failed" | "cancelled")) {
            return row;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    generation(app, id).await
}

#[tokio::test]
async fn a_late_routed_image_is_delivered_from_the_deployment_that_took_it() {
    // Replicate runs jobs and is served, but the route sends this one to the mock.
    let (app, state, replicate) = routed_app("replicate", "replicate/flux-schnell").await;
    let (status, _, body) = call(&app, routed_image("replicate/flux-schnell", "a routed lighthouse job_ms=100")).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
    let id = body["error"]["generation_id"].as_str().expect("the 504 names the generation").to_string();

    let done = until_terminal(&app, &state, &id).await;
    assert_eq!(done["status"], "completed", "the deployment's vendor finished the job: {done}");
    assert_eq!(done["metadata"]["assets"][0]["kind"], "image", "{done}");
    assert!((done["cost_usd"].as_f64().unwrap() - 0.003).abs() < 1e-9, "billed once: {done}");
    assert_eq!(done["provider"], "mock", "the row names the deployment that took the job: {done}");
    assert!(
        replicate.received_requests().await.unwrap_or_default().is_empty(),
        "the model's own vendor, which never had the job, was asked about it"
    );
}

#[tokio::test]
async fn a_routed_image_is_delivered_late_by_a_deployment_that_runs_jobs_though_its_models_vendor_does_not() {
    // fal answers in one call and can't be asked about a job again; the
    // route's mock deployment runs jobs, and took this one.
    let (app, state, fal) = routed_app("fal", "fal/flux-schnell").await;
    let (status, _, body) = call(&app, routed_image("fal/flux-schnell", "a routed harbour job_ms=100")).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT, "{body}");
    let id = body["error"]["generation_id"].as_str().expect("the deployment can deliver the job later, so the 504 names its generation").to_string();

    let done = until_terminal(&app, &state, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["provider"], "mock", "{done}");
    assert!(fal.received_requests().await.unwrap_or_default().is_empty(), "the route never sends this model to fal");
}

#[tokio::test]
async fn respond_async_is_honoured_on_a_route_whose_deployments_all_run_jobs() {
    // fal can't deliver a job late, but every deployment its route may send
    // the model to can, whichever takes the job.
    let (app, state, _fal) = routed_app("fal", "fal/flux-schnell").await;
    let mut req = routed_image("fal/flux-schnell", "a routed pier job_ms=100");
    req.headers_mut().insert("prefer", "respond-async".parse().unwrap());
    let (status, headers, body) = call(&app, req).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(headers.get("preference-applied").unwrap(), "respond-async");
    let done = until_terminal(&app, &state, body["id"].as_str().unwrap()).await;
    assert_eq!(done["status"], "completed", "{done}");
}

// ─── A caller's wait on a model route (routed-prefer, 2026-11-15) ───────────
// `Prefer: respond-async` / `wait=N` holds on a route as on the model's own
// vendor: an attempt at a deployment ends when the caller's wait does, and the
// job runs on at that deployment. The caller's wait running out is no failure
// of the deployment's vendor; the deployment's own time running out still is.

/// A deployment of `provider`, tried once, that litegen waits `timeout_seconds` for.
fn deployment(provider: &str, timeout_seconds: u64) -> litegen::config::DeploymentConfig {
    litegen::config::DeploymentConfig { provider: provider.into(), weight: 1, max_retries: 0, timeout_seconds, rpm_limit: 0 }
}

/// [`routed_app`] with the route's `deployments` (tried in order), and a
/// breaker that opens on a deployment's first failure.
async fn routed_app_with(
    own: &str,
    model: &str,
    deployments: Vec<litegen::config::DeploymentConfig>,
) -> (axum::Router, Arc<litegen::api::middleware::AppState>, wiremock::MockServer) {
    let vendor = wiremock::MockServer::start().await;
    let own_config = json!({ "enabled": true, "api_key": "PLATFORM_KEY", "api_base": vendor.uri() });
    let dir = tempfile::tempdir().unwrap().keep();
    let db = Arc::new(litegen::db::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let (app, state) = harness::app_and_state_over_with(db, &dir, |cfg| {
        cfg.providers.insert(own.to_string(), serde_json::from_value(own_config).unwrap());
        cfg.model_routes = vec![litegen::config::ModelRouteConfig { model: model.into(), deployments, strategy: None, cache: None }];
        cfg.circuit_breaker.threshold = 1;
    })
    .await;
    (app, state, vendor)
}

fn preferring(mut req: Request<Body>, prefer: &str) -> Request<Body> {
    req.headers_mut().insert("prefer", prefer.parse().unwrap());
    req
}

#[tokio::test]
async fn respond_async_on_a_route_answers_202_while_the_deployment_runs_the_job() {
    // The job takes 300 ms and the deployment may take 30 s; the caller waits
    // for nothing. Replicate, the next deployment, runs jobs too.
    let (app, state, replicate) = routed_app_with("replicate", "replicate/flux-schnell", vec![deployment("mock", 30), deployment("replicate", 30)]).await;
    let (status, headers, body) = call(&app, preferring(routed_image("replicate/flux-schnell", "a routed beacon job_ms=300"), "respond-async")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "the caller's wait, not the deployment's 30 s, ends the attempt: {} {}", body["provider"], body["error"]);
    assert_eq!(headers.get("preference-applied").unwrap(), "respond-async");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(generation(&app, &id).await["provider"], "mock", "the row names the deployment that took the job");

    let done = until_terminal(&app, &state, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    assert_eq!(done["metadata"]["assets"][0]["kind"], "image", "{done}");
    assert!(
        replicate.received_requests().await.unwrap_or_default().is_empty(),
        "the job runs on at the deployment that took it: the next one is never sent it"
    );
}

#[tokio::test]
async fn a_callers_wait_that_runs_out_on_a_route_is_no_failure_of_the_deployment() {
    // The breaker opens on a deployment's first failure (`routed_app_with`).
    let (app, state, _fal) = routed_app_with("fal", "fal/flux-schnell", vec![deployment("mock", 30)]).await;
    let (status, _, body) = call(&app, preferring(routed_image("fal/flux-schnell", "a routed buoy job_ms=100"), "respond-async")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert!(!state.router.circuit_breaker.is_open("mock").await, "the caller stopped waiting; the vendor is running the job");
}

#[tokio::test]
async fn a_deployments_own_timeout_still_counts_when_the_caller_would_wait_longer() {
    // litegen gives the deployment 0 s, less than the caller's 5 s: the
    // deployment's own time is what runs out.
    let (app, state, _fal) = routed_app_with("fal", "fal/flux-schnell", vec![deployment("mock", 0)]).await;
    let (status, _, body) = call(&app, preferring(routed_image("fal/flux-schnell", "a routed jetty job_ms=100"), "wait=5")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "the job is still delivered later: {body}");
    assert!(state.router.circuit_breaker.is_open("mock").await, "the deployment outlasted its own time: a failure, as without the header");
}

#[tokio::test]
async fn respond_async_is_not_honoured_on_a_route_with_a_deployment_that_cannot_deliver_late() {
    // The mock, tried first, runs jobs; fal, the next deployment, answers in
    // one call and could not deliver a job late. Which one takes the job
    // isn't known until the call runs, so the call is synchronous.
    let (app, _state, _fal) = routed_app_with("fal", "fal/flux-schnell", vec![deployment("mock", 30), deployment("fal", 30)]).await;
    let (status, headers, body) = call(&app, preferring(routed_image("fal/flux-schnell", "a routed quay job_ms=100"), "respond-async")).await;
    assert_eq!(status, StatusCode::OK, "{} {}", body["provider"], body["error"]);
    assert_eq!(body["provider"], "mock", "the deployment that runs jobs answered, once its job was done");
    assert!(headers.get("preference-applied").is_none(), "{headers:?}");
}
