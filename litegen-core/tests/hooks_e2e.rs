//! Hooks end to end, on the real `litegen` binary: a `litegen.yaml` declaring
//! the example hooks (`examples/hooks/`) in Python, Node and Bun, a server
//! started in that directory, and plain HTTP. Covers custom auth, rewrites,
//! tagging, async jobs polled to completion, multipart and the converter,
//! failures and recovery, the routes hooks never see, and startup refusals;
//! and stored-output access (`docs/storage-access.md`): signed links and the
//! `file_access` examples over real HTTP.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const MASTER: &str = "e2e-master";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn example(name: &str) -> PathBuf {
    repo().join("examples/hooks").join(name)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hooks").join(name)
}

fn installed(program: &str) -> bool {
    let ok = Command::new(program).arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    if !ok {
        eprintln!("SKIPPED: {program} not installed");
    }
    ok
}

/// A port that was free when asked and that no other server in this test
/// binary was given, so two servers starting at once never share one.
fn free_port() -> u16 {
    static GIVEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut given = GIVEN.lock().unwrap();
        if !given.contains(&port) {
            given.push(port);
            return port;
        }
    }
}

fn read_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("litegen.log")).unwrap_or_default()
}

/// A litegen binary running in its own directory; killed on drop.
struct Server {
    child: Child,
    base: String,
    dir: tempfile::TempDir,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn log(&self) -> String {
        read_log(self.dir.path())
    }
}

/// A directory with `litegen.yaml` and the given files under `hooks/`.
fn site(yaml: &str, files: &[PathBuf]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("litegen.yaml"), yaml).unwrap();
    std::fs::create_dir(dir.path().join("hooks")).unwrap();
    for f in files {
        std::fs::copy(f, dir.path().join("hooks").join(f.file_name().unwrap())).unwrap();
    }
    dir
}

fn command(dir: &Path, port: u16) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_litegen"));
    cmd.current_dir(dir)
        .env("LITEGEN__SERVER__HOST", "127.0.0.1")
        .env("LITEGEN__SERVER__PORT", port.to_string())
        .env("LITEGEN__DATABASE_URL", "sqlite://:memory:")
        .env("LITEGEN__MASTER_KEY", MASTER)
        .env("LITEGEN__PROVIDERS__MOCK__API_KEY", "")
        .env("LITEGEN_MODELS_DIR", repo().join("models"))
        // Read by the example hooks: inherited from litegen's environment.
        .env("LITEGEN_APP_KEY", MASTER)
        .env("ACME_TOKENS", "alice-token:alice,bob-token:bob")
        .env("ALLOWED_ORIGINS", "http://app.acme.test")
        .env("TEST_LITEGEN_KEY", MASTER)
        .env("RUST_LOG", "warn");
    cmd
}

async fn start(yaml: &str, files: &[PathBuf]) -> Server {
    start_on(yaml, files, std::iter::repeat_with(free_port)).await
}

/// `start`, on the first of `ports` litegen can bind. A port free when
/// free_port() named it can be taken by another process before litegen binds
/// it; litegen then exits on "Address already in use", and the next port is
/// tried.
async fn start_on(yaml: &str, files: &[PathBuf], mut ports: impl Iterator<Item = u16>) -> Server {
    let dir = site(yaml, files);
    // A probe that reaches something other than this litegen gives up in a second.
    let client = reqwest::Client::builder().timeout(Duration::from_secs(1)).build().unwrap();
    for _ in 0..5 {
        let port = ports.next().unwrap();
        let log = std::fs::File::create(dir.path().join("litegen.log")).unwrap();
        let mut child = command(dir.path(), port).stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();
        let base = format!("http://127.0.0.1:{port}");
        // Generous, as in `refuses`: under the whole suite's parallel load on a
        // 4-vCPU runner the debug binary is slow to load its hooks.
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let log = read_log(dir.path());
                assert!(log.contains("Address already in use"), "litegen exited ({status}) while starting:\n{log}");
                break;
            }
            if client.get(format!("{base}/health/live")).send().await.is_ok_and(|r| r.status().is_success()) {
                return Server { child, base, dir };
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("litegen did not come up:\n{}", read_log(dir.path()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    panic!("litegen found no port it could bind in 5 tries:\n{}", read_log(dir.path()));
}

#[tokio::test]
async fn a_server_whose_port_is_taken_starts_on_another() {
    // free_port()'s race, made certain: another process holds the first port
    // offered when litegen tries to bind it.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let ports = std::iter::once(port).chain(std::iter::repeat_with(free_port));
    let server = start_on("cost_markup_percent: 0\n", &[], ports).await;
    assert_ne!(server.base, format!("http://127.0.0.1:{port}"));
    assert_eq!(reqwest::get(format!("{}/health/live", server.base)).await.unwrap().status(), 200);
}

/// Start litegen expecting it to refuse: its exit status and its output.
/// The deadline is generous: under a full parallel test run the debug
/// binary is slow to reach its hooks, and a hook may take litegen's whole
/// 10 s load limit before litegen gives up on it.
fn refuses(yaml: &str, files: &[PathBuf]) -> (std::process::ExitStatus, String) {
    let dir = site(yaml, files);
    let mut child = command(dir.path(), free_port()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let out = child.wait_with_output().unwrap();
            return (status, format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("litegen kept running for 60 s with:\n{yaml}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

async fn call(server: &Server, method: reqwest::Method, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> (u16, reqwest::header::HeaderMap, Value) {
    let mut req = reqwest::Client::new().request(method, format!("{}{path}", server.base));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await.unwrap();
    let (status, headers) = (resp.status().as_u16(), resp.headers().clone());
    let bytes = resp.bytes().await.unwrap();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn poll(server: &Server, route: &str, id: &str, headers: &[(&str, &str)]) -> Value {
    for _ in 0..100 {
        let (status, _, body) = call(server, reqwest::Method::GET, &format!("{route}/{id}"), headers, None).await;
        assert_eq!(status, 200, "{body}");
        if body["status"] == "completed" || body["status"] == "failed" {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{route}/{id} never finished");
}

const CHAIN: &str = "hooks:
  - path: hooks/custom_auth.py
  - path: hooks/rewrite.ts
  - path: hooks/tag_results.mjs
";

fn chain_files() -> Vec<PathBuf> {
    vec![example("custom_auth.py"), example("rewrite.ts"), example("tag_results.mjs")]
}

fn all_installed() -> bool {
    installed("python3") && installed("node") && installed("bun")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mixed_chain_authenticates_rewrites_and_tags_a_real_generation() {
    if !all_installed() {
        return;
    }
    let server = start(CHAIN, &chain_files()).await;
    let (status, headers, body) = call(
        &server,
        reqwest::Method::POST,
        "/v1/images/generations",
        &[("x-acme-token", "alice-token")],
        Some(json!({ "model": "fast-image", "prompt": "a fox" })),
    )
    .await;
    assert_eq!(status, 200, "{body}\n{}", server.log());
    assert_eq!(body["served_by"], "litegen+hooks", "{body}");
    assert_eq!(headers.get("x-served-by").and_then(|v| v.to_str().ok()), Some("litegen+hooks"));

    let master = format!("Bearer {MASTER}");
    let id = body["id"].as_str().unwrap();
    let (_, _, row) = call(&server, reqwest::Method::GET, &format!("/v1/generations/{id}"), &[("authorization", &master)], None).await;
    assert_eq!(row["metadata"]["request"]["model"], "mock/visual-image-gen", "the Bun hook's alias: {row}");
    let (_, _, end_users) = call(&server, reqwest::Method::GET, "/v1/end-users", &[("authorization", &master)], None).await;
    assert!(end_users.to_string().contains("alice"), "the Python hook named the end user: {end_users}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_node_custom_auth_matches_the_python_one() {
    if !installed("node") {
        return;
    }
    let server = start("hooks:\n  - path: hooks/custom_auth.mjs\n", &[example("custom_auth.mjs")]).await;
    let image = json!({ "model": "mock/visual-image-gen", "prompt": "a fox" });
    let (status, _, body) = call(&server, reqwest::Method::POST, "/v1/images/generations", &[("x-acme-token", "bob-token")], Some(image.clone())).await;
    assert_eq!(status, 200, "{body}\n{}", server.log());
    let (status, headers, body) = call(&server, reqwest::Method::POST, "/v1/images/generations", &[("x-acme-token", "nope")], Some(image)).await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(headers.get("www-authenticate").and_then(|v| v.to_str().ok()), Some("AcmeToken"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_token_gets_the_hooks_401() {
    if !all_installed() {
        return;
    }
    let server = start(CHAIN, &chain_files()).await;
    let (status, headers, body) = call(
        &server,
        reqwest::Method::POST,
        "/v1/images/generations",
        &[("x-acme-token", "wrong")],
        Some(json!({ "model": "fast-image", "prompt": "a fox" })),
    )
    .await;
    assert_eq!(status, 401, "{body}");
    assert_eq!(body["error"]["code"], "refused_by_hook", "{body}");
    assert_eq!(body["error"]["hook"], "custom_auth.py", "{body}");
    assert_eq!(headers.get("www-authenticate").and_then(|v| v.to_str().ok()), Some("AcmeToken"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn video_and_3d_jobs_poll_to_completion_through_the_hooks() {
    if !all_installed() {
        return;
    }
    let server = start(CHAIN, &chain_files()).await;
    let alice = [("x-acme-token", "alice-token")];
    for (submit, model, route) in [("/v1/models3d/generations", "mock/mesh-3d", "/v1/models3d"), ("/v1/videos/generations", "mock/video-gen", "/v1/videos")] {
        let (status, _, body) = call(&server, reqwest::Method::POST, submit, &alice, Some(json!({ "model": model, "prompt": "a fox" }))).await;
        assert_eq!(status, 200, "{model}: {body}\n{}", server.log());
        let done = poll(&server, route, body["id"].as_str().unwrap(), &alice).await;
        assert_eq!(done["status"], "completed", "{model}: {done}");
        assert_eq!(done["served_by"], "litegen+hooks", "{model}: the finished answer was tagged: {done}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_multipart_and_converter_paths_work_through_the_hooks() {
    if !all_installed() {
        return;
    }
    let server = start(CHAIN, &chain_files()).await;
    let png = std::fs::read(repo().join("dashboard/.storybook/fixtures/image.png")).unwrap();
    let request = json!({ "model": "mock/mesh-3d", "prompt": "a fox", "reference_images": [{ "type": "blob", "value": "image_0", "role": "init" }] });
    let form = reqwest::multipart::Form::new()
        .part("request", reqwest::multipart::Part::text(request.to_string()).mime_str("application/json").unwrap())
        .part("image_0", reqwest::multipart::Part::bytes(png).file_name("image.png").mime_str("image/png").unwrap());
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/models3d/generations", server.base))
        .header("x-acme-token", "alice-token")
        .multipart(form)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    assert_eq!(status, 200, "{body}\n{}", server.log());
    let done = poll(&server, "/v1/models3d", body["id"].as_str().unwrap(), &[("x-acme-token", "alice-token")]).await;
    assert_eq!(done["status"], "completed", "{done}");

    let glb = litegen::providers::model3d::glb::generate_cube_glb("a cube");
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/utils/models3d/convert?from=glb&to=obj", server.base))
        .header("x-acme-token", "alice-token")
        .header("content-type", "model/gltf-binary")
        .body(glb)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers().get("x-served-by").and_then(|v| v.to_str().ok()), Some("litegen+hooks"));
    let obj = resp.text().await.unwrap();
    assert!(obj.lines().any(|l| l.starts_with("v ")), "an OBJ with vertices: {}", &obj[..obj.len().min(200)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn management_routes_ignore_the_hooks() {
    if !all_installed() {
        return;
    }
    let server = start(CHAIN, &chain_files()).await;
    let (status, _, body) = call(&server, reqwest::Method::GET, "/v1/keys", &[("x-acme-token", "alice-token")], None).await;
    assert_eq!(status, 401, "no hook injects a key on /v1/keys: {body}");
    let master = format!("Bearer {MASTER}");
    let (status, _, body) = call(&server, reqwest::Method::GET, "/v1/keys", &[("authorization", &master)], None).await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_timeout_and_a_crash_refuse_and_the_crash_recovers() {
    if !(installed("python3") && installed("node")) {
        return;
    }
    let yaml = "hooks:
  - path: hooks/custom_auth.py
  - path: hooks/behaviour.py
    timeout_ms: 200
  - path: hooks/behaviour.mjs
";
    let server = start(yaml, &[example("custom_auth.py"), fixture("behaviour.py"), fixture("behaviour.mjs")]).await;
    let image = json!({ "model": "mock/visual-image-gen", "prompt": "a fox" });
    async fn post(server: &Server, image: &Value, action: &str) -> (u16, reqwest::header::HeaderMap, Value) {
        let headers = [("x-acme-token", "alice-token"), ("x-test-action", action)];
        call(server, reqwest::Method::POST, "/v1/images/generations", &headers, Some(image.clone())).await
    }

    let (status, _, body) = post(&server, &image, "sleep").await;
    assert_eq!(status, 503, "{body}");
    assert_eq!((body["error"]["code"].as_str(), body["error"]["hook"].as_str()), (Some("hook_failed"), Some("behaviour.py")), "{body}");

    let (status, _, body) = post(&server, &image, "crash").await;
    assert_eq!(status, 503, "{body}");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (status, _, body) = post(&server, &image, "").await;
        if status == 200 {
            break;
        }
        assert!(Instant::now() < deadline, "not serving again within 5 s: {status} {body}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[test]
fn litegen_refuses_to_start_on_a_missing_or_broken_hook() {
    if !installed("python3") {
        return;
    }
    let (status, out) = refuses("hooks:\n  - path: hooks/missing.py\n", &[]);
    assert!(!status.success() && out.contains("hook file not found"), "{status}: {out}");

    let (status, out) = refuses("hooks:\n  - path: hooks/broken.py\n", &[fixture("broken.py")]);
    assert!(!status.success() && out.contains("broken.py") && out.contains("SyntaxError"), "{status}: {out}");

    let (status, out) = refuses("hooks:\n  - path: hooks/behaviour.py\n    runtime: cobol\n", &[fixture("behaviour.py")]);
    assert!(!status.success() && out.contains("defines hooks but did not load"), "{status}: {out}");

    // Valid hooks beside a config that won't load: the default-config fallback
    // would drop them, so litegen refuses instead.
    let (status, out) = refuses("cost_markup_percent: lots\nhooks:\n  - path: hooks/behaviour.py\n", &[fixture("behaviour.py")]);
    assert!(!status.success() && out.contains("defines hooks but did not load"), "{status}: {out}");
}

// ─── Stored outputs (docs/storage-access.md) ─────────────────────────────────

/// Each customer opens only the files made for them: custom auth names the end
/// user, litegen records it on the generation, and `file_access` compares.
async fn each_customer_opens_only_their_own_files(auth: &str, files: &str) {
    let yaml = format!("storage_access:\n  mode: private\nhooks:\n  - path: hooks/{auth}\n  - path: hooks/{files}\n");
    let server = start(&yaml, &[example(auth), example(files)]).await;
    let image = json!({ "model": "mock/visual-image-gen", "prompt": "alice's fox" });
    let (status, _, body) = call(&server, reqwest::Method::POST, "/v1/images/generations", &[("x-acme-token", "alice-token")], Some(image)).await;
    assert_eq!(status, 200, "{body}\n{}", server.log());
    let file = format!("/v1/generations/{}/assets/0", body["id"].as_str().unwrap());

    let (status, headers, _) = call(&server, reqwest::Method::GET, &file, &[("x-acme-token", "alice-token")], None).await;
    assert_eq!(status, 200, "{files}: alice opens her own file\n{}", server.log());
    assert_eq!(headers.get("content-type").and_then(|v| v.to_str().ok()), Some("image/png"));
    let (status, _, body) = call(&server, reqwest::Method::GET, &file, &[("x-acme-token", "bob-token")], None).await;
    assert_eq!(status, 403, "{files}: bob does not: {body}");
    assert_eq!((&body["error"]["code"], &body["error"]["hook"]), (&json!("refused_by_hook"), &json!(files)), "{body}");
    let (status, _, _) = call(&server, reqwest::Method::GET, &file, &[], None).await;
    assert_eq!(status, 401, "{files}: nobody else either");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_python_file_access_example_keeps_customers_to_their_own_files() {
    if installed("python3") {
        each_customer_opens_only_their_own_files("custom_auth.py", "file_access_end_user.py").await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_node_file_access_example_keeps_customers_to_their_own_files() {
    if installed("node") {
        each_customer_opens_only_their_own_files("custom_auth.mjs", "file_access_end_user.mjs").await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_links_open_files_over_real_http_until_they_expire() {
    let server = start("storage_access:\n  mode: signed\n  signed_url_ttl_seconds: 60\n", &[]).await;
    let auth = format!("Bearer {MASTER}");
    let (status, _, body) = call(&server, reqwest::Method::POST, "/v1/models3d/generations", &[("authorization", &auth)], Some(json!({ "model": "mock/mesh-3d", "prompt": "a chair" }))).await;
    assert_eq!(status, 200, "{body}");
    let done = poll(&server, "/v1/models3d", body["id"].as_str().unwrap(), &[("authorization", &auth)]).await;
    let link = done["assets"][0]["url"].as_str().unwrap().to_string();
    assert!(link.starts_with(&server.base) && link.contains("signature="), "{link}");
    let fetched = reqwest::get(&link).await.unwrap();
    assert_eq!(fetched.status(), 200, "a signed link needs no key");
    assert_eq!(&fetched.bytes().await.unwrap()[..4], b"glTF");
    assert_eq!(reqwest::get(link.split('?').next().unwrap()).await.unwrap().status(), 404, "the stored URL alone opens nothing");
    // The same link with its expiry moved is someone else's link.
    let (head, _) = link.split_once("expires=").unwrap();
    let rest = &link[head.len() + "expires=".len()..];
    let (expires, tail) = rest.split_once('&').unwrap();
    let later = format!("{head}expires={}&{tail}", expires.parse::<i64>().unwrap() + 3600);
    assert_eq!(reqwest::get(&later).await.unwrap().status(), 404, "an edited link is not a link");
}

#[test]
fn litegen_refuses_to_start_on_a_storage_access_it_cannot_read() {
    // In the file: the defaults (`public`) must never stand in for it.
    let (status, out) = refuses("storage_access:\n  mode: privat\n", &[]);
    assert!(!status.success() && out.contains("storage_access") && out.contains("privat"), "{status}: {out}");
    let (status, out) = refuses("storage_access:\n  signed_url_ttl_seconds: 30\n", &[]);
    assert!(!status.success() && out.contains("signed_url_ttl_seconds"), "{status}: {out}");
    // In the environment.
    let dir = site("cost_markup_percent: 0\n", &[]);
    let out = command(dir.path(), free_port()).env("LITEGEN__STORAGE_ACCESS__MODE", "privat").output().unwrap();
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success() && text.contains("storage_access"), "{}: {text}", out.status);
}

#[test]
fn litegen_refuses_to_start_on_any_config_it_cannot_read() {
    // A config that doesn't load used to be replaced by the defaults, which
    // dropped everything it set: the master key, the database, the cache.
    for (yaml, says) in [
        ("cache:\n  max_items: 0\n", "max_items"),
        ("retries:\n  max_retries: 9\n", "max_retries"),
        ("cost_markup_percent: lots\n", "cost_markup_percent"),
    ] {
        let (status, out) = refuses(yaml, &[]);
        assert!(!status.success() && out.contains("did not load") && out.contains(says), "{yaml}: {status}: {out}");
    }
}

/// The examples a guide quotes, each under a `<!-- example: <path> -->`
/// marker and checked against that file, whole.
fn quoted_examples(guide: &str) -> std::collections::BTreeSet<String> {
    let text = std::fs::read_to_string(repo().join(guide)).unwrap_or_else(|e| panic!("{guide}: {e}"));
    let lines: Vec<&str> = text.lines().collect();
    let mut quoted = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < lines.len() {
        let marker = lines[i].trim().strip_prefix("<!-- example: ").and_then(|r| r.strip_suffix(" -->"));
        let Some(path) = marker else {
            i += 1;
            continue;
        };
        assert!(lines.get(i + 1).is_some_and(|l| l.starts_with("```")), "{guide}: a fenced block must follow the marker for {path}");
        let mut j = i + 2;
        while j < lines.len() && !lines[j].starts_with("```") {
            j += 1;
        }
        let block = lines[i + 2..j].join("\n");
        let file = std::fs::read_to_string(repo().join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(block.trim_end(), file.trim_end(), "{guide} quotes {path} exactly");
        quoted.insert(path.to_string());
        i = j + 1;
    }
    quoted
}

/// Every example in `docs/hooks.md` sits under a `<!-- example: <path> -->`
/// marker and quotes that file whole, so the guide can't drift from the
/// examples these tests run. `docs/storage-access.md` quotes one too.
#[test]
fn the_guide_quotes_every_example_verbatim() {
    let quoted = quoted_examples("docs/hooks.md");
    let hook_file = |p: &Path| p.is_file() && p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["py", "js", "mjs", "cjs", "ts", "mts"].contains(&e));
    let examples: std::collections::BTreeSet<String> = std::fs::read_dir(repo().join("examples/hooks"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| hook_file(p))
        .map(|p| format!("examples/hooks/{}", p.file_name().unwrap().to_string_lossy()))
        .collect();
    assert_eq!(quoted, examples, "every example is quoted in the guide");
    let storage = quoted_examples("docs/storage-access.md");
    assert!(storage.contains("examples/hooks/file_access_end_user.py"), "{storage:?}");
}
