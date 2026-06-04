//! Real-HTTP multi-tenant integration suite.
//!
//! Unlike the in-process `oneshot`/`axum-test` unit tests, every test here boots
//! the *real* server (`litegen::api::create_router` + `axum::serve`) on an
//! ephemeral `127.0.0.1:0` TCP port and drives it with a `reqwest` HTTP client
//! over the wire. Each test gets a fresh on-disk SQLite database (a tempfile, so
//! a single pooled connection sees a shared DB — `sqlite::memory:` would give
//! each pooled connection its own empty DB) and runs the server in HOSTED mode
//! with a master key + secrets key. No state is shared between tests.
//!
//! Cookies: `reqwest`'s `cookies` feature is NOT enabled in this crate's deps,
//! so this suite uses a tiny manual cookie jar (`Client`) that captures
//! `set-cookie` from responses and replays a `Cookie` header on later requests.

use std::sync::Arc;

use litegen::api::create_router;
use litegen::api::middleware::AppState;
use litegen::capabilities::CapabilityRegistry;
use litegen::config::{AppConfig, CacheGlobalConfig, DevFlags, Mode};
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use litegen::proxy::cache::GenerationCache;
use litegen::proxy::materializer::{MaterializeError, Materializer, TempStorage};
use litegen::proxy::registry::ProviderRegistry;
use litegen::proxy::router::ProxyRouter;
use litegen::proxy::storage::{ImageStore, LocalStore};

use bytes::Bytes;
use serde_json::{json, Value};

const MASTER_KEY: &str = "test-master";
const MOCK_MODEL: &str = "mock/image-gen";
const PASSWORD: &str = "correct-horse-battery"; // >= 12 chars

// ─── Temp storage that does nothing (mirrors the NoopStorage in unit tests) ───

struct NoopStorage;

#[async_trait::async_trait]
impl TempStorage for NoopStorage {
    async fn put(&self, key: &str, _bytes: Bytes, _ct: &str) -> Result<String, MaterializeError> {
        Ok(format!("local://{}", key))
    }
    async fn delete(&self, _key: &str) -> Result<(), MaterializeError> {
        Ok(())
    }
}

// ─── Test app: real server on a real port + fresh DB ───────────────────────────

struct TestApp {
    base: String,
    // Keep the tempfile alive for the duration of the test; dropping it deletes
    // the backing SQLite file.
    _tmp: tempfile::NamedTempFile,
}

impl TestApp {
    fn db_path(&self) -> std::path::PathBuf {
        self._tmp.path().to_path_buf()
    }
}

async fn spawn_app() -> TestApp {
    spawn_app_with_providers(&[]).await
}

/// Like [`spawn_app`] but registers additional global provider instances beyond
/// the default `mock`. Each entry is `(name, config_json)` and is merged into
/// `AppConfig::providers` before `init_from_config`, so e.g. a real provider can
/// be pointed at a `wiremock` upstream. (Non-`mock` providers still need *some*
/// credential present, or `init_from_config` skips them — pass a placeholder
/// `api_key`; a per-app BYO credential overrides it per request.)
async fn spawn_app_with_providers(extra_providers: &[(&str, Value)]) -> TestApp {
    spawn_app_cfg(extra_providers, Some([7u8; 32])).await
}

/// Like [`spawn_app`] but with `secrets_key: None` — exercises the
/// `secrets_not_configured` error path in storage/credential handlers.
async fn spawn_app_no_secrets() -> TestApp {
    spawn_app_cfg(&[], None).await
}

async fn spawn_app_cfg(extra_providers: &[(&str, Value)], secrets_key: Option<[u8; 32]>) -> TestApp {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let url = format!("sqlite://{}?mode=rwc", tmp.path().display());
    let db: Arc<dyn DatabaseStore> = Arc::new(
        SqliteDatabase::connect(&url)
            .await
            .expect("connect + migrate sqlite"),
    );

    // Provider registry with the mock image+video providers registered (the mock
    // needs no credentials). `ProviderRegistry::new()` starts empty; the
    // `register_mock_*` helpers used by the in-crate unit tests are `#[cfg(test)]`
    // and so are NOT visible from an integration-test crate. Instead we register
    // the mock the public way: add a `mock` provider entry to the config and run
    // `init_from_config`, which has a credential-exempt `"mock"` arm.
    let mut app_config = AppConfig::default();
    app_config.providers.insert(
        "mock".to_string(),
        serde_json::from_value(serde_json::json!({ "enabled": true }))
            .expect("mock provider config"),
    );
    for (name, cfg) in extra_providers {
        app_config.providers.insert(
            (*name).to_string(),
            serde_json::from_value(cfg.clone()).expect("extra provider config"),
        );
    }
    let provider_registry = Arc::new(ProviderRegistry::new());
    provider_registry.init_from_config(&app_config).await;

    let config = Arc::new(app_config);
    let cache = Arc::new(GenerationCache::new(&CacheGlobalConfig::default()));
    let image_store: Arc<dyn ImageStore> = Arc::new(LocalStore);
    let router = Arc::new(ProxyRouter::new(provider_registry, cache, config, image_store));

    let materializer = Arc::new(Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));

    // Shipped capability registry lives at <repo>/models, i.e. CARGO_MANIFEST_DIR
    // (litegen-core) with the last component popped, then `models`. Mirrors the
    // `build_test_state()` recipe in src/api/handlers/mod.rs.
    let mut models_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    models_dir.pop();
    models_dir.push("models");
    let cap_registry =
        Arc::new(CapabilityRegistry::from_dir(&models_dir).expect("load shipped models"));

    let state = Arc::new(AppState {
        router,
        db,
        master_key: Some(MASTER_KEY.to_string()),
        registry: cap_registry,
        materializer,
        rate_limiter: Arc::new(litegen::api::middleware::rate_limit::RateLimiter::new()),
        in_flight: Arc::new(litegen::api::middleware::backpressure::InFlightLimit::new(64)),
        oauth: litegen::auth::oauth::OAuthConfig::default(),
        mode: Mode::Hosted,
        allow_password: true,
        secrets_key,
        dev: DevFlags {
            expose_invite_tokens: true,
            expose_reset_tokens: true,
        },
    });

    // Cookies are sent over plaintext HTTP in tests; without this the `Secure`
    // attribute is harmless for our manual jar (we ignore attributes), but set
    // it anyway to mirror a realistic dev deployment.
    std::env::set_var("LITEGEN__COOKIE_INSECURE_DEV", "true");

    let app = create_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestApp {
        base: format!("http://{addr}"),
        _tmp: tmp,
    }
}

// ─── Minimal HTTP client with a manual cookie jar ──────────────────────────────

/// A thin wrapper over `reqwest::Client` that records `set-cookie` name=value
/// pairs and replays them as a `Cookie` header. `reqwest`'s built-in cookie
/// store is not compiled in (the `cookies` feature is off), so we do it by hand.
struct Client {
    http: reqwest::Client,
    base: String,
    cookies: std::collections::HashMap<String, String>,
}

struct Resp {
    status: u16,
    body: Value,
}

impl Client {
    fn new(base: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: base.to_string(),
            cookies: std::collections::HashMap::new(),
        }
    }

    fn cookie_header(&self) -> Option<String> {
        if self.cookies.is_empty() {
            return None;
        }
        Some(
            self.cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }

    fn capture_cookies(&mut self, resp: &reqwest::Response) {
        for hv in resp.headers().get_all("set-cookie").iter() {
            if let Ok(s) = hv.to_str() {
                // first segment is name=value; ignore attributes after ';'
                let first = s.split(';').next().unwrap_or("");
                if let Some((name, value)) = first.split_once('=') {
                    let name = name.trim().to_string();
                    let value = value.trim().to_string();
                    if value.is_empty() {
                        // Max-Age=0 clears the cookie.
                        self.cookies.remove(&name);
                    } else {
                        self.cookies.insert(name, value);
                    }
                }
            }
        }
    }

    async fn send(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        extra_headers: &[(&str, &str)],
    ) -> Resp {
        let url = format!("{}{}", self.base, path);
        let mut req = self.http.request(method, &url);
        if let Some(c) = self.cookie_header() {
            req = req.header("cookie", c);
        }
        for (k, v) in extra_headers {
            req = req.header(*k, *v);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.expect("request send");
        self.capture_cookies(&resp);
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let body = serde_json::from_str(&text).unwrap_or(Value::Null);
        Resp { status, body }
    }

    async fn get(&mut self, path: &str) -> Resp {
        self.send(reqwest::Method::GET, path, None, &[]).await
    }
    async fn get_with(&mut self, path: &str, headers: &[(&str, &str)]) -> Resp {
        self.send(reqwest::Method::GET, path, None, headers).await
    }
    async fn post(&mut self, path: &str, body: Value) -> Resp {
        self.send(reqwest::Method::POST, path, Some(body), &[]).await
    }
    async fn post_with(&mut self, path: &str, body: Value, headers: &[(&str, &str)]) -> Resp {
        self.send(reqwest::Method::POST, path, Some(body), headers)
            .await
    }
    async fn delete(&mut self, path: &str, headers: &[(&str, &str)]) -> Resp {
        self.send(reqwest::Method::DELETE, path, None, headers).await
    }
    async fn put_with(&mut self, path: &str, body: Value, headers: &[(&str, &str)]) -> Resp {
        self.send(reqwest::Method::PUT, path, Some(body), headers).await
    }

    /// Fetch the CSRF token for the current session.
    async fn csrf(&mut self) -> String {
        let r = self.get("/v1/auth/csrf").await;
        assert_eq!(r.status, 200, "csrf fetch failed: {:?}", r.body);
        r.body["csrf_token"]
            .as_str()
            .expect("csrf_token string")
            .to_string()
    }
}

// ─── Helpers ───────────────────────────────────────────────────────────────────

fn unique_email(tag: &str) -> String {
    format!("{}-{}@example.com", tag, uuid::Uuid::new_v4())
}

/// Sign up (hosted mode), returning the active org id from `/v1/orgs`.
async fn signup(client: &mut Client, email: &str, org_name: &str) -> Resp {
    client
        .post(
            "/v1/auth/signup",
            json!({ "email": email, "password": PASSWORD, "org_name": org_name }),
        )
        .await
}

async fn first_org_id(client: &mut Client) -> String {
    let r = client.get("/v1/orgs").await;
    assert_eq!(r.status, 200, "list orgs failed: {:?}", r.body);
    r.body[0]["id"].as_str().expect("org id").to_string()
}

// ─── Test 1: signup creates an org + a default app ─────────────────────────────

#[tokio::test]
async fn signup_creates_org_and_app() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let email = unique_email("acme");

    let r = signup(&mut c, &email, "Acme").await;
    assert_eq!(r.status, 200, "signup failed: {:?}", r.body);
    // signup set the session cookie
    assert!(c.cookies.contains_key("litegen_session"), "session cookie not set");

    let orgs = c.get("/v1/orgs").await;
    assert_eq!(orgs.status, 200);
    let arr = orgs.body.as_array().expect("orgs array");
    assert_eq!(arr.len(), 1, "expected exactly one org: {:?}", orgs.body);
    assert_eq!(arr[0]["name"], "Acme");

    let org_id = arr[0]["id"].as_str().unwrap().to_string();
    let apps = c.get(&format!("/v1/orgs/{org_id}/apps")).await;
    assert_eq!(apps.status, 200, "list apps failed: {:?}", apps.body);
    let apps_arr = apps.body.as_array().expect("apps array");
    assert!(!apps_arr.is_empty(), "expected at least one app");
}

// ─── Test 2: two signups → isolated orgs ───────────────────────────────────────

#[tokio::test]
async fn two_signups_create_isolated_orgs() {
    let app = spawn_app().await;
    let mut a = Client::new(&app.base);
    let mut b = Client::new(&app.base);

    signup(&mut a, &unique_email("a"), "OrgA").await;
    signup(&mut b, &unique_email("b"), "OrgB").await;

    let a_orgs = a.get("/v1/orgs").await;
    let a_names: Vec<&str> = a_orgs
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(a_names, vec!["OrgA"]);
    assert!(!a_names.contains(&"OrgB"), "A must not see B's org");

    let b_orgs = b.get("/v1/orgs").await;
    let b_names: Vec<&str> = b_orgs
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(b_names, vec!["OrgB"]);
}

// ─── Test 3: CSRF required on mutating session requests ────────────────────────

#[tokio::test]
async fn session_csrf_required() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    signup(&mut c, &unique_email("csrf"), "CsrfOrg").await;

    // POST /v1/orgs is a mutating session route guarded by csrf_middleware.
    let no_token = c.post("/v1/orgs", json!({ "name": "Second" })).await;
    assert_eq!(
        no_token.status, 403,
        "expected 403 without CSRF token, got {} {:?}",
        no_token.status, no_token.body
    );

    let token = c.csrf().await;
    let with_token = c
        .post_with("/v1/orgs", json!({ "name": "Second" }), &[("x-csrf-token", &token)])
        .await;
    assert!(
        (200..300).contains(&with_token.status),
        "expected 2xx with CSRF token, got {} {:?}",
        with_token.status,
        with_token.body
    );
}

// ─── Test 4: key creation returns id + secret once; list never returns secret ──

#[tokio::test]
async fn create_key_returns_id_and_secret_once() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    signup(&mut c, &unique_email("keys"), "KeyOrg").await;
    let token = c.csrf().await;

    let created = c
        .post_with(
            "/v1/keys",
            json!({ "name": "ci-key", "scopes": "generate,read" }),
            &[("x-csrf-token", &token)],
        )
        .await;
    assert_eq!(created.status, 201, "create key failed: {:?}", created.body);

    let public_id = created.body["public_id"].as_str().expect("public_id");
    let secret = created.body["key"].as_str().expect("key (secret)");
    assert!(
        public_id.starts_with("pk_live_"),
        "public_id should start pk_live_: {public_id}"
    );
    assert!(
        secret.starts_with("sk_live_"),
        "secret should start sk_live_: {secret}"
    );

    let list = c.get("/v1/keys").await;
    assert_eq!(list.status, 200, "list keys failed: {:?}", list.body);
    let data = list.body["data"].as_array().expect("data array");
    assert_eq!(data.len(), 1, "expected one key");
    let entry = &data[0];
    assert_eq!(entry["public_id"].as_str(), Some(public_id));
    assert!(entry.get("prefix").is_some(), "list entry should expose prefix");
    // The raw secret must NEVER appear in a list response.
    assert!(
        entry.get("key").is_none(),
        "list entry must not contain the raw secret: {entry:?}"
    );
    assert!(
        !list.body.to_string().contains(secret),
        "raw secret leaked in list response"
    );
}

// ─── Test 5: Bearer secret can generate ────────────────────────────────────────

#[tokio::test]
async fn bearer_secret_generates() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    signup(&mut c, &unique_email("gen"), "GenOrg").await;
    let token = c.csrf().await;

    let created = c
        .post_with(
            "/v1/keys",
            json!({ "name": "gen-key", "scopes": "generate,read" }),
            &[("x-csrf-token", &token)],
        )
        .await;
    assert_eq!(created.status, 201, "create key failed: {:?}", created.body);
    let secret = created.body["key"].as_str().unwrap().to_string();

    // A fresh client (no cookies) authenticated purely by Bearer secret.
    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "generation failed: {:?}", r.body);
}

// ─── Test 6: cross-tenant isolation (the crux) ─────────────────────────────────

#[tokio::test]
async fn cross_tenant_isolation() {
    let app = spawn_app().await;

    // Org A: signs up, creates a key, generates via the key.
    let mut a = Client::new(&app.base);
    signup(&mut a, &unique_email("orga"), "OrgA").await;
    let a_org_id = first_org_id(&mut a).await;
    let a_csrf = a.csrf().await;
    let a_key = a
        .post_with(
            "/v1/keys",
            json!({ "name": "a-key", "scopes": "generate,read" }),
            &[("x-csrf-token", &a_csrf)],
        )
        .await;
    assert_eq!(a_key.status, 201, "A create key failed: {:?}", a_key.body);
    let a_secret = a_key.body["key"].as_str().unwrap().to_string();
    let a_public_id = a_key.body["public_id"].as_str().unwrap().to_string();

    // A generates using its key (Bearer).
    let mut a_bearer = Client::new(&app.base);
    let a_bearer_hdr = format!("Bearer {a_secret}");
    let gen = a_bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "secret-a-prompt" }),
            &[("authorization", &a_bearer_hdr)],
        )
        .await;
    assert_eq!(gen.status, 200, "A generation failed: {:?}", gen.body);

    // Org B: separate tenant.
    let mut b = Client::new(&app.base);
    signup(&mut b, &unique_email("orgb"), "OrgB").await;

    // B's key list must NOT include A's key.
    let b_keys = b.get("/v1/keys").await;
    assert_eq!(b_keys.status, 200, "B list keys failed: {:?}", b_keys.body);
    let b_key_data = b_keys.body["data"].as_array().unwrap();
    assert!(
        b_key_data
            .iter()
            .all(|k| k["public_id"].as_str() != Some(a_public_id.as_str())),
        "B must not see A's key: {:?}",
        b_keys.body
    );

    // B's generations list must be empty (cannot see A's generation). This is
    // deterministic regardless of A's async-persist timing.
    let b_gens = b.get("/v1/generations").await;
    assert_eq!(b_gens.status, 200, "B list generations failed: {:?}", b_gens.body);
    assert_eq!(
        b_gens.body["total"].as_u64(),
        Some(0),
        "B must not see any generations: {:?}",
        b_gens.body
    );

    // B sending A's org id via X-Litegen-Org-Id on a session request → 403
    // (B is not a member of A's org). The auth middleware validates membership.
    let cross = b
        .get_with("/v1/orgs", &[("x-litegen-org-id", a_org_id.as_str())])
        .await;
    assert_eq!(
        cross.status, 403,
        "B forging A's org id should be 403, got {} {:?}",
        cross.status, cross.body
    );
}

// ─── Test 7: invite flow + role enforcement ────────────────────────────────────

#[tokio::test]
async fn invite_and_role_enforcement() {
    let app = spawn_app().await;

    // Owner signs up.
    let mut owner = Client::new(&app.base);
    signup(&mut owner, &unique_email("owner"), "InviteOrg").await;
    let org_id = first_org_id(&mut owner).await;
    let owner_csrf = owner.csrf().await;

    // Owner invites a member; dev.expose_invite_tokens=true → token in `_dev_token`.
    let member_email = unique_email("member");
    let invite = owner
        .post_with(
            &format!("/v1/orgs/{org_id}/members"),
            json!({ "email": member_email, "role": "member" }),
            &[("x-csrf-token", &owner_csrf)],
        )
        .await;
    assert_eq!(invite.status, 200, "invite failed: {:?}", invite.body);
    let token = invite.body["_dev_token"]
        .as_str()
        .expect("dev invite token")
        .to_string();

    // Accept the invitation (creates the member account + a session).
    let mut member = Client::new(&app.base);
    let accept = member
        .post(
            &format!("/v1/auth/invitations/{token}/accept"),
            json!({ "password": PASSWORD }),
        )
        .await;
    assert_eq!(accept.status, 200, "accept invite failed: {:?}", accept.body);
    assert!(
        member.cookies.contains_key("litegen_session"),
        "member session cookie not set after accept"
    );

    // Member tries to invite another user → 403 (member lacks member:invite).
    let member_csrf = member.csrf().await;
    let member_invite = member
        .post_with(
            &format!("/v1/orgs/{org_id}/members"),
            json!({ "email": unique_email("nope"), "role": "member" }),
            &[("x-csrf-token", &member_csrf)],
        )
        .await;
    assert_eq!(
        member_invite.status, 403,
        "member inviting should be 403, got {} {:?}",
        member_invite.status, member_invite.body
    );

    // Owner can still invite.
    let owner_csrf2 = owner.csrf().await;
    let owner_invite = owner
        .post_with(
            &format!("/v1/orgs/{org_id}/members"),
            json!({ "email": unique_email("second"), "role": "member" }),
            &[("x-csrf-token", &owner_csrf2)],
        )
        .await;
    assert_eq!(
        owner_invite.status, 200,
        "owner inviting should succeed: {:?}",
        owner_invite.body
    );
}

// ─── Test 8: revoked key is rejected ───────────────────────────────────────────

#[tokio::test]
async fn revoked_key_rejected() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    signup(&mut c, &unique_email("revoke"), "RevokeOrg").await;
    let csrf = c.csrf().await;

    let created = c
        .post_with(
            "/v1/keys",
            json!({ "name": "doomed", "scopes": "generate,read" }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(created.status, 201, "create key failed: {:?}", created.body);
    let key_id = created.body["id"].as_str().expect("key id").to_string();
    let secret = created.body["key"].as_str().unwrap().to_string();

    // Sanity: the key works before revocation.
    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let before = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(before.status, 200, "pre-revoke generation failed: {:?}", before.body);

    // Revoke as the owner session (DELETE is CSRF-guarded).
    let csrf2 = c.csrf().await;
    let del = c
        .delete(&format!("/v1/keys/{key_id}"), &[("x-csrf-token", &csrf2)])
        .await;
    assert!(
        (200..300).contains(&del.status),
        "revoke failed: {} {:?}",
        del.status,
        del.body
    );

    // Bearer with the revoked secret → 401.
    let after = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(
        after.status, 401,
        "revoked key should be 401, got {} {:?}",
        after.status, after.body
    );
}

// ─── Test 9: hosted master key cannot read tenant data ─────────────────────────

#[tokio::test]
async fn hosted_master_key_cannot_read_tenant_data() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);

    // In hosted mode the master key is a platform admin bound to NO org/app, so
    // any tenant-scoped route returns 403 (no active organization). There is no
    // /v1/admin/* route in this build, so we just assert the 403 on /v1/keys.
    let master_hdr = format!("Bearer {MASTER_KEY}");
    let r = c
        .get_with("/v1/keys", &[("authorization", &master_hdr)])
        .await;
    assert_eq!(
        r.status, 403,
        "hosted master key should be 403 on tenant data, got {} {:?}",
        r.status, r.body
    );
}

// ─── Test 10: quota exhausted → 402 on the next request ────────────────────────
//
// The non-zero-cost `mock/expensive-image` model (base_cost_usd = $5.00 on the
// credential-free `mock` provider) lets us exercise the real quota path:
//   * `generate_image` charges `tokens_used += cost` AFTER a successful 200
//     (only when cost > 0.0; the charge itself never fails on over-quota — it
//     just increments — so the first request returns a clean 200).
//   * The hard 402 is enforced PRE-FLIGHT in `handle_db_key` on the NEXT request
//     when `tokens_used >= token_quota` (middleware mod.rs:404-417).
// With token_quota = $1.00, one $5 generation pushes tokens_used to $5 ≥ $1, so
// the second identical request is rejected 402 before it ever reaches the
// handler.

const MOCK_EXPENSIVE_MODEL: &str = "mock/expensive-image";

#[tokio::test]
async fn quota_exhausted_402() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    signup(&mut c, &unique_email("quota"), "QuotaOrg").await;
    let csrf = c.csrf().await;

    // A key with a $1.00 USD budget — one $5 generation blows past it.
    let created = c
        .post_with(
            "/v1/keys",
            json!({ "name": "quota-key", "token_quota": 1.0, "scopes": "generate,read" }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(created.status, 201, "create key failed: {:?}", created.body);
    let secret = created.body["key"].as_str().unwrap().to_string();

    // Fresh cookieless client authenticated purely by Bearer secret.
    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let body = json!({ "model": MOCK_EXPENSIVE_MODEL, "prompt": "x" });

    // 1st request: succeeds (200), charges $5 → now over quota.
    let first = bearer
        .post_with("/v1/images/generations", body.clone(), &[("authorization", &bearer_hdr)])
        .await;
    assert_eq!(
        first.status, 200,
        "first generation should succeed (charge is post-success): {} {:?}",
        first.status, first.body
    );

    // 2nd identical request: pre-flight quota check rejects with 402.
    let second = bearer
        .post_with("/v1/images/generations", body, &[("authorization", &bearer_hdr)])
        .await;
    assert_eq!(
        second.status, 402,
        "second generation should be 402 (quota exhausted), got {} {:?}",
        second.status, second.body
    );
}

// ─── Test 11: a read-only key cannot generate (scope enforcement) ──────────────
//
// `/v1/images/generations` sits behind a `check_scope(Scope::Generate)` layer; a
// key minted with only the `read` scope is rejected 403. A read route
// (`GET /v1/generations`, behind `check_scope(Scope::Read)`) is a positive
// control: the same key CAN reach it.

#[tokio::test]
async fn read_only_key_cannot_generate_403() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    signup(&mut c, &unique_email("readonly"), "ReadOnlyOrg").await;
    let csrf = c.csrf().await;

    let created = c
        .post_with(
            "/v1/keys",
            json!({ "name": "read-key", "scopes": "read" }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(created.status, 201, "create key failed: {:?}", created.body);
    let secret = created.body["key"].as_str().unwrap().to_string();

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");

    // Negative: generate is blocked by the Generate-scope layer → 403.
    let gen = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(
        gen.status, 403,
        "read-only key must not generate, got {} {:?}",
        gen.status, gen.body
    );

    // Positive control: a read route is reachable with the read scope.
    let gens = bearer
        .get_with("/v1/generations", &[("authorization", &bearer_hdr)])
        .await;
    assert_eq!(
        gens.status, 200,
        "read-only key should reach a read route, got {} {:?}",
        gens.status, gens.body
    );
}

// ─── Test 12: forging another org's app id via header → 403 ────────────────────
//
// On every session request `auth_middleware` resolves the active app: if an
// `X-Litegen-App-Id` header is present it must belong to the active org,
// otherwise the request is rejected 403 (mod.rs:243-250). Client A (org A)
// sending B's app id — with no org header, so A's active org is its own — must
// be rejected because B's app does not belong to A's org.

#[tokio::test]
async fn app_id_forge_rejected() {
    let app = spawn_app().await;

    // Client A (org A).
    let mut a = Client::new(&app.base);
    signup(&mut a, &unique_email("forge-a"), "ForgeOrgA").await;

    // Client B (org B) — capture B's default app id.
    let mut b = Client::new(&app.base);
    signup(&mut b, &unique_email("forge-b"), "ForgeOrgB").await;
    let b_org = first_org_id(&mut b).await;
    let b_apps = b.get(&format!("/v1/orgs/{b_org}/apps")).await;
    assert_eq!(b_apps.status, 200, "B list apps failed: {:?}", b_apps.body);
    let b_app_id = b_apps.body.as_array().expect("apps array")[0]["id"]
        .as_str()
        .expect("b app id")
        .to_string();

    // A sends B's app id on a session request (no org header → A's own org is
    // active). B's app does not belong to A's org → 403.
    let forged = a
        .get_with("/v1/orgs", &[("x-litegen-app-id", b_app_id.as_str())])
        .await;
    assert_eq!(
        forged.status, 403,
        "A forging B's app id should be 403, got {} {:?}",
        forged.status, forged.body
    );

    // Sanity: without the forged header A's own session works fine.
    let ok = a.get("/v1/orgs").await;
    assert_eq!(ok.status, 200, "A's own session should work: {:?}", ok.body);
}

// ─── Test 13: cross-tenant key detail is 404, never 403 or the key ─────────────
//
// `GET /v1/keys/{id}` returns 404 (not 403, not the key) when the key belongs to
// another org — the "indistinguishable from non-existent" path (mod.rs:1104).

#[tokio::test]
async fn cross_tenant_key_detail_404() {
    let app = spawn_app().await;

    // Client A creates a key; capture its uuid `id`.
    let mut a = Client::new(&app.base);
    signup(&mut a, &unique_email("ctd-a"), "CtdOrgA").await;
    let a_csrf = a.csrf().await;
    let a_key = a
        .post_with(
            "/v1/keys",
            json!({ "name": "a-detail-key", "scopes": "generate,read" }),
            &[("x-csrf-token", &a_csrf)],
        )
        .await;
    assert_eq!(a_key.status, 201, "A create key failed: {:?}", a_key.body);
    let a_key_id = a_key.body["id"].as_str().expect("key uuid id").to_string();

    // Client B (separate org) tries to read A's key by id → 404.
    let mut b = Client::new(&app.base);
    signup(&mut b, &unique_email("ctd-b"), "CtdOrgB").await;
    let detail = b.get(&format!("/v1/keys/{a_key_id}")).await;
    assert_eq!(
        detail.status, 404,
        "B reading A's key should be 404 (indistinguishable), got {} {:?}",
        detail.status, detail.body
    );

    // A can still read its own key (positive control).
    let own = a.get(&format!("/v1/keys/{a_key_id}")).await;
    assert_eq!(own.status, 200, "A reading its own key should be 200: {:?}", own.body);
}

// ─── Test 13b: cross-tenant webhook-deliveries is 404 ──────────────────────────
//
// `GET /v1/keys/{id}/webhook-deliveries` must be tenant-scoped: org B reading
// org A's key's deliveries → 404 (indistinguishable from non-existent). Org A
// reading its own key's deliveries → 200 (empty list).

#[tokio::test]
async fn cross_tenant_webhook_deliveries_404() {
    let app = spawn_app().await;

    // Org A creates a key; capture its uuid `id`.
    let mut a = Client::new(&app.base);
    signup(&mut a, &unique_email("cwd-a"), "CwdOrgA").await;
    let a_csrf = a.csrf().await;
    let a_key = a
        .post_with(
            "/v1/keys",
            json!({ "name": "a-wh-key", "scopes": "generate,read" }),
            &[("x-csrf-token", &a_csrf)],
        )
        .await;
    assert_eq!(a_key.status, 201, "A create key failed: {:?}", a_key.body);
    let a_key_id = a_key.body["id"].as_str().expect("key uuid id").to_string();

    // Org B (separate org/session) tries to read A's key's deliveries → 404.
    let mut b = Client::new(&app.base);
    signup(&mut b, &unique_email("cwd-b"), "CwdOrgB").await;
    let cross = b.get(&format!("/v1/keys/{a_key_id}/webhook-deliveries")).await;
    assert_eq!(
        cross.status, 404,
        "B reading A's webhook deliveries should be 404 (indistinguishable), got {} {:?}",
        cross.status, cross.body
    );

    // A can read its own key's deliveries (positive control; empty is fine).
    let own = a.get(&format!("/v1/keys/{a_key_id}/webhook-deliveries")).await;
    assert_eq!(
        own.status, 200,
        "A reading its own webhook deliveries should be 200: {:?}",
        own.body
    );
}

// ─── Test 13c: cross-tenant test-webhook is 404 ────────────────────────────────
//
// `POST /v1/keys/{id}/test-webhook` must be tenant-scoped: org B triggering a
// test webhook for org A's key → 404 (never 200/400, so B cannot fire A's
// webhook or probe key existence). For the owner the route is reachable: with no
// webhook configured it returns 400 `no_webhook_configured` — asserted NOT 404.

#[tokio::test]
async fn cross_tenant_test_webhook_404() {
    let app = spawn_app().await;

    // Org A creates a key (no webhook_url); capture its uuid `id`.
    let mut a = Client::new(&app.base);
    signup(&mut a, &unique_email("ctw-a"), "CtwOrgA").await;
    let a_csrf = a.csrf().await;
    let a_key = a
        .post_with(
            "/v1/keys",
            json!({ "name": "a-tw-key", "scopes": "generate,read" }),
            &[("x-csrf-token", &a_csrf)],
        )
        .await;
    assert_eq!(a_key.status, 201, "A create key failed: {:?}", a_key.body);
    let a_key_id = a_key.body["id"].as_str().expect("key uuid id").to_string();

    // Org B (separate org/session) tries to fire a test webhook for A's key → 404.
    let mut b = Client::new(&app.base);
    signup(&mut b, &unique_email("ctw-b"), "CtwOrgB").await;
    let b_csrf = b.csrf().await;
    let cross = b
        .post_with(
            &format!("/v1/keys/{a_key_id}/test-webhook"),
            json!({}),
            &[("x-csrf-token", &b_csrf)],
        )
        .await;
    assert_eq!(
        cross.status, 404,
        "B firing A's test webhook should be 404 (indistinguishable), got {} {:?}",
        cross.status, cross.body
    );

    // The owner (A) reaches the route: no webhook configured → not a 404.
    let own = a
        .post_with(
            &format!("/v1/keys/{a_key_id}/test-webhook"),
            json!({}),
            &[("x-csrf-token", &a_csrf)],
        )
        .await;
    assert_ne!(
        own.status, 404,
        "A firing its own key's test webhook must NOT be 404 (owner reaches route), got {} {:?}",
        own.status, own.body
    );
}

// ─── Test 14: logout invalidates the session ───────────────────────────────────
//
// After signup the session cookie authenticates `GET /v1/auth/me` (200). POST
// `/v1/auth/logout` is CSRF-guarded; it deletes the session server-side and
// clears the cookie (Max-Age=0, which our jar drops). A subsequent
// `GET /v1/auth/me` with the now-cleared jar reaches the hosted-mode auth
// fallback → 401.

#[tokio::test]
async fn logout_invalidates_session() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    signup(&mut c, &unique_email("logout"), "LogoutOrg").await;
    assert!(
        c.cookies.contains_key("litegen_session"),
        "session cookie not set after signup"
    );

    // Authenticated before logout.
    let me_before = c.get("/v1/auth/me").await;
    assert_eq!(me_before.status, 200, "me before logout: {:?}", me_before.body);

    // Logout (CSRF-guarded mutating route).
    let csrf = c.csrf().await;
    let out = c
        .post_with("/v1/auth/logout", json!({}), &[("x-csrf-token", &csrf)])
        .await;
    assert!(
        (200..300).contains(&out.status),
        "logout should succeed (2xx), got {} {:?}",
        out.status,
        out.body
    );
    // The Max-Age=0 clear should have dropped the session cookie from the jar.
    assert!(
        !c.cookies.contains_key("litegen_session"),
        "session cookie should be cleared after logout"
    );

    // The (now-cleared) jar can no longer authenticate.
    let me_after = c.get("/v1/auth/me").await;
    assert_eq!(
        me_after.status, 401,
        "me after logout should be 401, got {} {:?}",
        me_after.status, me_after.body
    );
}

// ─── BYO provider credentials (Phase 2) ────────────────────────────────────────

/// Sign up + create a `generate,read` Bearer key bound to the org's default app.
/// Returns `(default_app_id, bearer_secret)`. The session `Client` `c` keeps its
/// cookies so it can still mutate the org (e.g. store a provider credential).
async fn signup_app_and_key(c: &mut Client, tag: &str) -> (String, String) {
    signup(c, &unique_email(tag), &format!("{tag}-org")).await;
    let org_id = first_org_id(c).await;
    let apps = c.get(&format!("/v1/orgs/{org_id}/apps")).await;
    assert_eq!(apps.status, 200, "list apps failed: {:?}", apps.body);
    let app_id = apps.body.as_array().expect("apps array")[0]["id"]
        .as_str()
        .expect("default app id")
        .to_string();

    let csrf = c.csrf().await;
    let created = c
        .post_with(
            "/v1/keys",
            json!({ "name": format!("{tag}-key"), "scopes": "generate,read" }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(created.status, 201, "create key failed: {:?}", created.body);
    let secret = created.body["key"].as_str().expect("key secret").to_string();
    (app_id, secret)
}

// ─── Test: BYO — real provider, no app cred, no global instance → 400 ──────────
//
// The harness registers only the `mock` provider globally, so a *real* provider
// (`openai`) has no global instance, and a fresh app has stored no BYO credential.
// Generating against a real-provider model must therefore surface the documented
// `provider_not_configured` 400 (NOT a 5xx, and NOT an upstream attempt).
#[tokio::test]
async fn byo_missing_credential_returns_400() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (_app_id, secret) = signup_app_and_key(&mut c, "byo-missing").await;

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            // `openai/dall-e-3` is a real, non-mock provider model shipped in models/.
            json!({ "model": "openai/dall-e-3", "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;

    assert_eq!(
        r.status, 400,
        "expected 400 provider_not_configured, got {} {:?}",
        r.status, r.body
    );
    assert_eq!(
        r.body["error"]["type"].as_str(),
        Some("provider_not_configured"),
        "unexpected error body: {:?}",
        r.body
    );
}

// ─── Test: BYO — mock still works with no credential ───────────────────────────
//
// The mock provider needs no credential and is registered globally, so the no-cred
// path must still return 200 (we didn't break the existing single_tenant/mock flow).
#[tokio::test]
async fn mock_generation_still_works_without_cred() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (_app_id, secret) = signup_app_and_key(&mut c, "byo-mock").await;

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;

    assert_eq!(
        r.status, 200,
        "mock generation should still succeed with no credential, got {} {:?}",
        r.status, r.body
    );
}

// ─── Test: BYO — stored app credential reaches the upstream ────────────────────
//
// Stand up a `wiremock` upstream that returns a minimal valid DALL-E b64 response.
// Register `openai` globally pointed at that upstream with a *placeholder* key (so
// the global instance exists but its key is NOT what we assert on). Store an app
// provider-credential with `api_key=APPKEY` via the owner session, then generate
// with the Bearer key requesting `response_format=b64_json` (the b64 path needs no
// follow-up image fetch). Assert the upstream received `Authorization: Bearer APPKEY`
// — i.e. the per-app BYO credential, not the platform placeholder, reached upstream.
#[tokio::test]
async fn byo_credential_reaches_upstream() {
    use base64::Engine as _;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let upstream = MockServer::start().await;
    let fake_b64 = base64::engine::general_purpose::STANDARD.encode(b"fake-png-bytes");
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "created": 1234567890,
            "data": [{ "b64_json": fake_b64 }]
        })))
        .expect(1)
        .mount(&upstream)
        .await;

    // openai registered globally → its api_base points at the wiremock upstream.
    // The placeholder api_key keeps `init_from_config` from skipping the provider;
    // the per-app BYO credential overrides it per request.
    let openai_cfg = json!({
        "enabled": true,
        "api_key": "PLATFORM_PLACEHOLDER",
        "api_base": format!("{}/v1/images/generations", upstream.uri()),
    });
    let app = spawn_app_with_providers(&[("openai", openai_cfg)]).await;

    let mut c = Client::new(&app.base);
    let (app_id, secret) = signup_app_and_key(&mut c, "byo-upstream").await;

    // Store the app's BYO credential (api_key=APPKEY) via the owner session.
    let csrf = c.csrf().await;
    let stored = c
        .post_with(
            &format!("/v1/apps/{app_id}/provider-credentials"),
            json!({ "provider": "openai", "credentials": { "api_key": "APPKEY" } }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(stored.status, 200, "store provider credential failed: {:?}", stored.body);

    // Generate with the Bearer key; b64_json avoids an upstream image fetch.
    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": "openai/dall-e-3", "prompt": "x", "response_format": "b64_json" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "BYO generation should succeed, got {} {:?}", r.status, r.body);

    // The upstream must have seen the per-app key, not the platform placeholder.
    let received = upstream.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 1, "expected exactly one upstream call");
    let auth = received[0]
        .headers
        .get("authorization")
        .expect("authorization header present")
        .to_str()
        .expect("authorization header utf-8");
    assert_eq!(
        auth, "Bearer APPKEY",
        "upstream should receive the per-app BYO key, got {auth:?}"
    );
}

// ─── BYO app storage: API surface ────────────────────────────────────────────

#[tokio::test]
async fn app_storage_crud_roundtrip_and_no_secret_leak() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, _secret) = signup_app_and_key(&mut c, "stg-crud").await;
    let csrf = c.csrf().await;

    let g0 = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g0.status, 200, "{:?}", g0.body);
    assert_eq!(g0.body["configured"], serde_json::json!(false));

    let put = c
        .put_with(
            &format!("/v1/apps/{app_id}/storage"),
            json!({
                "bucket_name": "acme-bucket",
                "region": "us-west-2",
                "endpoint_url": "https://minio.example.com",
                "path_prefix": "litegen/images",
                "access_key_id": "AKIAEXAMPLE9999",
                "secret_access_key": "top-secret-value"
            }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(put.status, 200, "put storage failed: {:?}", put.body);
    // PUT echoes the stored config (not just status) — guards against a {}-body regression.
    assert_eq!(put.body["configured"], serde_json::json!(true));
    assert_eq!(put.body["bucket_name"], "acme-bucket");

    let g1 = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g1.body["configured"], serde_json::json!(true));
    assert_eq!(g1.body["bucket_name"], "acme-bucket");
    assert_eq!(g1.body["region"], "us-west-2");
    assert_eq!(g1.body["access_key_id_hint"], "…9999");
    let raw = serde_json::to_string(&g1.body).unwrap();
    assert!(!raw.contains("top-secret-value"), "secret leaked in GET: {raw}");
    assert!(!raw.contains("AKIAEXAMPLE9999"), "access key id leaked in GET: {raw}");

    let del = c.delete(&format!("/v1/apps/{app_id}/storage"), &[("x-csrf-token", &csrf)]).await;
    assert_eq!(del.status, 204, "{:?}", del.body);
    let g2 = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g2.body["configured"], serde_json::json!(false));
    // Deleting an already-absent config is a 404.
    let del2 = c.delete(&format!("/v1/apps/{app_id}/storage"), &[("x-csrf-token", &csrf)]).await;
    assert_eq!(del2.status, 404, "second delete should 404: {:?}", del2.body);
}

#[tokio::test]
async fn app_storage_upsert_retains_secret_when_keys_omitted() {
    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, _secret) = signup_app_and_key(&mut c, "stg-retain").await;
    let csrf = c.csrf().await;

    let p1 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b1", "access_key_id": "AKIA1111", "secret_access_key": "sek" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p1.status, 200, "{:?}", p1.body);

    let p2 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b2" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p2.status, 200, "retain-secret put failed: {:?}", p2.body);
    let g = c.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(g.body["bucket_name"], "b2");
    assert_eq!(g.body["access_key_id_hint"], "…1111");

    let p3 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b3", "access_key_id": "AKIA2222" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p3.status, 400, "incomplete creds should 400: {:?}", p3.body);

    let _ = c.delete(&format!("/v1/apps/{app_id}/storage"), &[("x-csrf-token", &csrf)]).await;
    let p4 = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({ "bucket_name": "b4" }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(p4.status, 400, "create without keys should 400: {:?}", p4.body);
}

#[tokio::test]
async fn app_storage_cross_tenant_isolation() {
    let app = spawn_app().await;

    let mut a = Client::new(&app.base);
    let (app_a, _) = signup_app_and_key(&mut a, "stg-orga").await;
    let csrf_a = a.csrf().await;
    let put = a.put_with(
        &format!("/v1/apps/{app_a}/storage"),
        json!({ "bucket_name": "a-bucket", "access_key_id": "AKIAAAAA", "secret_access_key": "sek" }),
        &[("x-csrf-token", &csrf_a)],
    ).await;
    assert_eq!(put.status, 200, "{:?}", put.body);

    let mut b = Client::new(&app.base);
    signup(&mut b, &unique_email("stg-orgb"), "OrgB").await;
    let csrf_b = b.csrf().await;

    let g = b.get(&format!("/v1/apps/{app_a}/storage")).await;
    assert!(g.status == 403 || g.status == 404, "cross-tenant GET leaked: {} {:?}", g.status, g.body);
    let p = b.put_with(
        &format!("/v1/apps/{app_a}/storage"),
        json!({ "bucket_name": "hijack", "access_key_id": "x", "secret_access_key": "y" }),
        &[("x-csrf-token", &csrf_b)],
    ).await;
    assert!(p.status == 403 || p.status == 404, "cross-tenant PUT allowed: {} {:?}", p.status, p.body);
    let d = b.delete(&format!("/v1/apps/{app_a}/storage"), &[("x-csrf-token", &csrf_b)]).await;
    assert!(d.status == 403 || d.status == 404, "cross-tenant DELETE allowed: {} {:?}", d.status, d.body);
}

// ─── BYO app storage: generation behavior ────────────────────────────────────

#[tokio::test]
async fn app_storage_uploads_generation_to_per_app_bucket() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let s3 = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s3)
        .await;

    let app = spawn_app().await; // global image_store is LocalStore (b64 fallback)
    let mut c = Client::new(&app.base);
    let (app_id, secret) = signup_app_and_key(&mut c, "stg-upload").await;
    let csrf = c.csrf().await;

    let put = c
        .put_with(
            &format!("/v1/apps/{app_id}/storage"),
            json!({
                "bucket_name": "test-bucket",
                "region": "us-east-1",
                "endpoint_url": s3.uri(),
                "custom_public_url": "https://cdn.example.test",
                "path_prefix": "litegen/images",
                "access_key_id": "AKIATEST",
                "secret_access_key": "secret"
            }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(put.status, 200, "put storage failed: {:?}", put.body);

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "a cat" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "generation failed: {:?}", r.body);

    let url = r.body["data"][0]["url"].as_str().unwrap_or("");
    assert!(
        url.starts_with("https://cdn.example.test/litegen/images/"),
        "expected per-app bucket URL, got {url:?} (body {:?})",
        r.body
    );

    let received = s3.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 1, "expected one S3 PUT, got {}", received.len());
    let p = received[0].url.path();
    assert!(
        p.starts_with("/test-bucket/litegen/images/"),
        "unexpected S3 key path: {p}"
    );
}

#[tokio::test]
async fn unconfigured_app_falls_back_to_global_store() {
    let app = spawn_app().await; // global LocalStore => b64 inline
    let mut c = Client::new(&app.base);
    let (_app_id, secret) = signup_app_and_key(&mut c, "stg-nofb").await;

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert!(r.body["data"][0]["b64_json"].is_string(), "expected b64 fallback: {:?}", r.body);
    assert!(r.body["data"][0]["url"].is_null(), "expected no url: {:?}", r.body);
}

#[tokio::test]
async fn corrupt_storage_config_falls_back_without_failing() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let s3 = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s3)
        .await;

    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, secret) = signup_app_and_key(&mut c, "stg-corrupt").await;
    let csrf = c.csrf().await;

    let put = c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({
            "bucket_name": "test-bucket",
            "endpoint_url": s3.uri(),
            "access_key_id": "AKIATEST",
            "secret_access_key": "secret"
        }),
        &[("x-csrf-token", &csrf)],
    ).await;
    assert_eq!(put.status, 200, "{:?}", put.body);

    let url = format!("sqlite://{}?mode=rwc", app.db_path().display());
    let pool = sqlx::sqlite::SqlitePool::connect(&url).await.expect("open sqlite");
    sqlx::query("UPDATE app_storage_credentials SET secret_ciphertext = 'not-base64-$$$' WHERE app_id = ?")
        .bind(&app_id)
        .execute(&pool)
        .await
        .expect("corrupt row");
    pool.close().await;

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r = bearer
        .post_with(
            "/v1/images/generations",
            json!({ "model": MOCK_MODEL, "prompt": "x" }),
            &[("authorization", &bearer_hdr)],
        )
        .await;
    assert_eq!(r.status, 200, "corrupt config must fail-open, got {:?}", r.body);
    assert!(r.body["data"][0]["b64_json"].is_string(), "expected b64 fallback: {:?}", r.body);
    let received = s3.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 0, "corrupt config must NOT hit per-app S3");
}

#[tokio::test]
async fn delete_storage_reverts_to_global_store() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let s3 = MockServer::start().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&s3)
        .await;

    let app = spawn_app().await;
    let mut c = Client::new(&app.base);
    let (app_id, secret) = signup_app_and_key(&mut c, "stg-revert").await;
    let csrf = c.csrf().await;

    c.put_with(
        &format!("/v1/apps/{app_id}/storage"),
        json!({
            "bucket_name": "test-bucket",
            "endpoint_url": s3.uri(),
            "custom_public_url": "https://cdn.example.test",
            "access_key_id": "AKIATEST",
            "secret_access_key": "secret"
        }),
        &[("x-csrf-token", &csrf)],
    ).await;

    let mut bearer = Client::new(&app.base);
    let bearer_hdr = format!("Bearer {secret}");
    let r1 = bearer.post_with(
        "/v1/images/generations",
        json!({ "model": MOCK_MODEL, "prompt": "x" }),
        &[("authorization", &bearer_hdr)],
    ).await;
    assert_eq!(r1.status, 200, "{:?}", r1.body);
    assert!(r1.body["data"][0]["url"].as_str().unwrap_or("").starts_with("https://cdn.example.test/"));

    let del = c.delete(&format!("/v1/apps/{app_id}/storage"), &[("x-csrf-token", &csrf)]).await;
    assert_eq!(del.status, 204, "{:?}", del.body);
    let r2 = bearer.post_with(
        "/v1/images/generations",
        json!({ "model": MOCK_MODEL, "prompt": "y" }),
        &[("authorization", &bearer_hdr)],
    ).await;
    assert_eq!(r2.status, 200, "{:?}", r2.body);
    assert!(r2.body["data"][0]["b64_json"].is_string(), "expected b64 after delete: {:?}", r2.body);

    let received = s3.received_requests().await.expect("recorded requests");
    assert_eq!(received.len(), 1, "exactly one S3 PUT total (before delete)");
}

// ─── BYO app storage: secrets_key required for PUT ───────────────────────────
//
// When the server is started without a `secrets_key`, the PUT handler must
// return 400 `secrets_not_configured` immediately (it cannot encrypt credentials
// without the key). The error lives at `body["error"]["code"]` per the `err()`
// helper in orgs.rs.

#[tokio::test]
async fn app_storage_put_requires_secrets_key() {
    let app = spawn_app_no_secrets().await;
    let mut c = Client::new(&app.base);
    let (app_id, _secret) = signup_app_and_key(&mut c, "stg-nokey").await;
    let csrf = c.csrf().await;

    let put = c
        .put_with(
            &format!("/v1/apps/{app_id}/storage"),
            json!({ "bucket_name": "b", "access_key_id": "AKIA", "secret_access_key": "s" }),
            &[("x-csrf-token", &csrf)],
        )
        .await;
    assert_eq!(put.status, 400, "expected 400 without secrets key: {:?}", put.body);
    // Error code is nested at body["error"]["code"] per the orgs.rs `err()` helper.
    let code = put.body["error"]["code"].as_str().unwrap_or("");
    assert_eq!(code, "secrets_not_configured", "unexpected error body: {:?}", put.body);
}

// ─── BYO app storage: Viewer role is read-only ───────────────────────────────
//
// A Viewer has `StorageCredRead` (GET 200) but not `StorageCredWrite` (PUT 403)
// or `StorageCredDelete` (DELETE 403). This test invites a Viewer using the same
// invite→accept→login flow as `invite_and_role_enforcement`.

#[tokio::test]
async fn app_storage_viewer_read_only() {
    let app = spawn_app().await;

    // Owner creates an app with storage configured.
    let mut owner = Client::new(&app.base);
    let (app_id, _secret) = signup_app_and_key(&mut owner, "stg-viewer-org").await;
    let org_id = first_org_id(&mut owner).await;
    let owner_csrf = owner.csrf().await;

    // PUT storage so the app is configured (gives the viewer something to read).
    let put = owner
        .put_with(
            &format!("/v1/apps/{app_id}/storage"),
            json!({
                "bucket_name": "viewer-bucket",
                "access_key_id": "AKIAVIEWER",
                "secret_access_key": "viewer-secret"
            }),
            &[("x-csrf-token", &owner_csrf)],
        )
        .await;
    assert_eq!(put.status, 200, "owner put storage failed: {:?}", put.body);

    // Owner invites a Viewer; dev.expose_invite_tokens=true → token in `_dev_token`.
    let viewer_email = unique_email("viewer");
    let invite = owner
        .post_with(
            &format!("/v1/orgs/{org_id}/members"),
            json!({ "email": viewer_email, "role": "viewer" }),
            &[("x-csrf-token", &owner_csrf)],
        )
        .await;
    assert_eq!(invite.status, 200, "invite viewer failed: {:?}", invite.body);
    let token = invite.body["_dev_token"]
        .as_str()
        .expect("dev invite token")
        .to_string();

    // Accept the invitation (creates the viewer account + a session).
    let mut viewer = Client::new(&app.base);
    let accept = viewer
        .post(
            &format!("/v1/auth/invitations/{token}/accept"),
            json!({ "password": PASSWORD }),
        )
        .await;
    assert_eq!(accept.status, 200, "accept invite failed: {:?}", accept.body);
    assert!(
        viewer.cookies.contains_key("litegen_session"),
        "viewer session cookie not set after accept"
    );

    let viewer_csrf = viewer.csrf().await;

    // GET /v1/apps/{app_id}/storage → 200 (Viewer has StorageCredRead).
    let get = viewer.get(&format!("/v1/apps/{app_id}/storage")).await;
    assert_eq!(
        get.status, 200,
        "viewer should be able to GET storage (StorageCredRead), got {} {:?}",
        get.status, get.body
    );

    // PUT /v1/apps/{app_id}/storage → 403 (Viewer lacks StorageCredWrite).
    let put_viewer = viewer
        .put_with(
            &format!("/v1/apps/{app_id}/storage"),
            json!({ "bucket_name": "hijack", "access_key_id": "AKIA", "secret_access_key": "s" }),
            &[("x-csrf-token", &viewer_csrf)],
        )
        .await;
    assert_eq!(
        put_viewer.status, 403,
        "viewer must not PUT storage (lacks StorageCredWrite), got {} {:?}",
        put_viewer.status, put_viewer.body
    );

    // DELETE /v1/apps/{app_id}/storage → 403 (Viewer lacks StorageCredDelete).
    let del_viewer = viewer
        .delete(
            &format!("/v1/apps/{app_id}/storage"),
            &[("x-csrf-token", &viewer_csrf)],
        )
        .await;
    assert_eq!(
        del_viewer.status, 403,
        "viewer must not DELETE storage (lacks StorageCredDelete), got {} {:?}",
        del_viewer.status, del_viewer.body
    );
}
