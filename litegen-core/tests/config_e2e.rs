//! litegen's configuration end to end, on the real binary
//! (`docs/configuration.md`): where it finds its config file, how a file is
//! named, `os.environ/` references, which of two settings wins, and values
//! that look like numbers.
//!
//! Each server runs in its own directory with a cleared environment, so a
//! developer's own `OPENAI_API_KEY` or `AWS_*` can't change a result, and is
//! bound to 127.0.0.1. Nothing else is needed: the database is SQLite in
//! memory (or a file in the server's directory), and the one vendor is
//! wiremock on 127.0.0.1.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const MASTER: &str = "e2e-master";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
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

/// A directory holding `files`, each a relative path and its text.
fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

/// litegen in `dir`, with only the environment a test gives it.
fn command(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_litegen"));
    cmd.current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("LITEGEN_MODELS_DIR", repo().join("models"))
        .env("LITEGEN__DATABASE_URL", "sqlite://:memory:")
        .env("RUST_LOG", "warn");
    cmd
}

/// `command` on 127.0.0.1:`port`, with the master key.
fn served(dir: &Path, port: u16) -> Command {
    let mut cmd = command(dir);
    cmd.env("LITEGEN__SERVER__HOST", "127.0.0.1")
        .env("LITEGEN__SERVER__PORT", port.to_string())
        .env("LITEGEN__MASTER_KEY", MASTER);
    cmd
}

/// `command` with only the port set: the host and the key come from wherever
/// the test puts them.
fn on_port(dir: &Path, port: u16) -> Command {
    let mut cmd = command(dir);
    cmd.env("LITEGEN__SERVER__PORT", port.to_string());
    cmd
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// A litegen that came up; killed on drop.
struct Server {
    child: Child,
    base: String,
    log: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn log(&self) -> String {
        read(&self.log)
    }
}

enum Started {
    Up(Server),
    /// It exited: its status and everything it wrote.
    Exited(ExitStatus, String),
}

impl Started {
    fn up(self, what: &str) -> Server {
        match self {
            Started::Up(server) => server,
            Started::Exited(status, out) => panic!("{what}: litegen must start, but it exited ({status}):\n{out}"),
        }
    }

    fn exited(self, what: &str) -> (ExitStatus, String) {
        match self {
            Started::Exited(status, out) => (status, out),
            Started::Up(server) => panic!("{what}: litegen must refuse to start, but it came up:\n{}", server.log()),
        }
    }
}

/// Run `make(port)`, logging to `dir/litegen.log`, until it serves
/// /health/live or exits (60 s at most). A port free when free_port() named it
/// can be taken before litegen binds it; litegen then exits on "Address
/// already in use", and `make` is called again with another port, as
/// hooks_e2e's `start_on` does.
async fn start(dir: &Path, mut make: impl FnMut(u16) -> Command) -> Started {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(1)).build().unwrap();
    let log_path = dir.join("litegen.log");
    for _ in 0..5 {
        let port = free_port();
        let log = std::fs::File::create(&log_path).unwrap();
        let mut cmd = make(port);
        let mut child = cmd.stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();
        let base = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let out = read(&log_path);
                if out.contains("Address already in use") {
                    break;
                }
                return Started::Exited(status, out);
            }
            if client.get(format!("{base}/health/live")).send().await.is_ok_and(|r| r.status().is_success()) {
                return Started::Up(Server { child, base, log: log_path });
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("litegen neither came up nor exited within 60 s:\n{}", read(&log_path));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    panic!("litegen found no port it could bind in 5 tries:\n{}", read(&log_path));
}

/// The status of `GET /v1/keys` with `key` as the bearer.
async fn keys_status(server: &Server, key: &str) -> u16 {
    reqwest::Client::new().get(format!("{}/v1/keys", server.base)).bearer_auth(key).send().await.unwrap().status().as_u16()
}

/// A vendor that records each call and turns it away.
async fn recording_vendor() -> MockServer {
    let vendor = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({ "error": { "message": "recorded", "type": "invalid_request_error" } })))
        .mount(&vendor)
        .await;
    vendor
}

/// The `Authorization` litegen sent `vendor` for one image generation.
async fn key_sent(server: &Server, vendor: &MockServer) -> String {
    let _ = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", server.base))
        .bearer_auth(MASTER)
        .json(&json!({ "model": "openai/gpt-image-2", "prompt": "a red cube" }))
        .send()
        .await
        .unwrap();
    let requests = vendor.received_requests().await.unwrap_or_default();
    let last = requests.last().unwrap_or_else(|| panic!("the vendor was never called:\n{}", server.log()));
    last.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string()
}

// ─── S1: the config file is found by name, never by the binary's ────────────

#[tokio::test]
async fn litegen_yaml_is_read_beside_the_binary_named_litegen() {
    // The image's layout: WORKDIR /app holds the binary /app/litegen, and an
    // empty file of that name is all it takes. No master key: only the file's
    // 127.0.0.1 lets litegen start; ignored, it would bind 0.0.0.0 and refuse.
    let dir = site(&[("litegen", ""), ("litegen.yaml", "server:\n  host: \"127.0.0.1\"\n")]);
    start(dir.path(), |port| on_port(dir.path(), port)).await.up("litegen.yaml beside a file named litegen");
}

#[tokio::test]
async fn a_hook_or_storage_access_beside_the_binary_is_never_dropped() {
    let dir = site(&[("litegen", ""), ("litegen.yaml", "hooks:\n  - path: hooks/missing.py\n")]);
    let (status, out) = start(dir.path(), |port| served(dir.path(), port)).await.exited("a hook beside litegen");
    assert!(!status.success() && out.contains("hook file not found"), "{status}: {out}");

    let dir = site(&[("litegen", ""), ("litegen.yaml", "storage_access:\n  mode: privat\n")]);
    let (status, out) = start(dir.path(), |port| served(dir.path(), port)).await.exited("storage_access beside litegen");
    assert!(!status.success() && out.contains("storage_access") && out.contains("privat"), "{status}: {out}");
}

// ─── S5: naming the file, and os.environ/ references ────────────────────────

#[tokio::test]
async fn the_config_flag_and_variables_name_the_file() {
    // Neither a host nor a key in the environment: only the named file binds
    // 127.0.0.1, so litegen starts only if it read it.
    let dir = site(&[("conf/site.yaml", "server:\n  host: \"127.0.0.1\"\n")]);
    let ways: [(&str, &[&str], Option<&str>); 5] = [
        ("--config <file>", &["--config", "conf/site.yaml"], None),
        ("--config=<file>", &["--config=conf/site.yaml"], None),
        ("-c <file>", &["-c", "conf/site.yaml"], None),
        ("LITEGEN_CONFIG_FILE_PATH", &[], Some("LITEGEN_CONFIG_FILE_PATH")),
        ("LITEGEN_CONFIG_FILE", &[], Some("LITEGEN_CONFIG_FILE")),
    ];
    for (what, args, variable) in ways {
        let server = start(dir.path(), |port| {
            let mut cmd = on_port(dir.path(), port);
            cmd.args(args).env("RUST_LOG", "info");
            if let Some(variable) = variable {
                cmd.env(variable, "conf/site.yaml");
            }
            cmd
        })
        .await
        .up(what);
        assert!(server.log().contains("conf/site.yaml"), "{what}: the log must name the file read:\n{}", server.log());
    }
}

#[tokio::test]
async fn a_named_config_file_that_is_missing_refuses_to_start() {
    let dir = site(&[]);
    let (status, out) = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.args(["--config", "nope.yaml"]);
        cmd
    })
    .await
    .exited("--config nope.yaml");
    assert!(!status.success() && out.contains("nope.yaml") && out.contains("--config"), "{status}: {out}");

    let (status, out) = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("LITEGEN_CONFIG_FILE", "nope.yaml");
        cmd
    })
    .await
    .exited("LITEGEN_CONFIG_FILE=nope.yaml");
    assert!(!status.success() && out.contains("nope.yaml") && out.contains("LITEGEN_CONFIG_FILE"), "{status}: {out}");
}

#[tokio::test]
async fn an_unknown_argument_refuses_to_start() {
    // LiteLLM's --port: silently ignored, it would leave litegen on 4000.
    let dir = site(&[]);
    let (status, out) = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.args(["--port", "5000"]);
        cmd
    })
    .await
    .exited("--port 5000");
    assert_eq!(status.code(), Some(2), "{out}");
    assert!(out.contains("--port") && out.contains("--config"), "{out}");
}

#[tokio::test]
async fn help_prints_the_usage_and_exits() {
    let dir = site(&[]);
    let (status, out) = start(dir.path(), |port| {
        let mut cmd = on_port(dir.path(), port);
        cmd.arg("--help");
        cmd
    })
    .await
    .exited("--help");
    assert!(status.success(), "{status}: {out}");
    assert!(out.contains("--config") && out.contains("LITEGEN_CONFIG_FILE_PATH"), "{out}");
}

#[tokio::test]
async fn a_hook_path_starts_in_the_config_files_folder() {
    // LiteLLM's rule: beside the file read, which for a litegen.yaml found in
    // the working directory is that directory.
    let dir = site(&[("conf/site.yaml", "hooks:\n  - path: hooks/missing.py\n")]);
    let (status, out) = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.args(["--config", "conf/site.yaml"]);
        cmd
    })
    .await
    .exited("a hook in a named file");
    assert!(!status.success() && out.contains("hook file not found") && out.contains("conf/hooks/missing.py"), "{status}: {out}");
}

#[tokio::test]
async fn os_environ_references_take_the_variables_text() {
    let dir = site(&[("litegen.yaml", "server:\n  host: \"127.0.0.1\"\nmaster_key: os.environ/E2E_MASTER_KEY\n")]);
    let server = start(dir.path(), |port| {
        let mut cmd = on_port(dir.path(), port);
        cmd.env("E2E_MASTER_KEY", "0012345");
        cmd
    })
    .await
    .up("master_key: os.environ/E2E_MASTER_KEY");
    assert_eq!(keys_status(&server, "0012345").await, 200, "{}", server.log());
}

#[tokio::test]
async fn an_os_environ_reference_to_an_unset_variable_refuses_to_start() {
    let dir = site(&[("litegen.yaml", "server:\n  host: \"127.0.0.1\"\nmaster_key: os.environ/E2E_UNSET_KEY\n")]);
    let (status, out) = start(dir.path(), |port| on_port(dir.path(), port)).await.exited("an unset reference");
    assert!(!status.success() && out.contains("master_key") && out.contains("E2E_UNSET_KEY"), "{status}: {out}");
}

// ─── S4: values are taken as written ────────────────────────────────────────

#[tokio::test]
async fn a_master_key_of_digits_is_kept_as_written() {
    let dir = site(&[]);
    let server = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("LITEGEN__MASTER_KEY", "0012345");
        cmd
    })
    .await
    .up("LITEGEN__MASTER_KEY=0012345");
    assert_eq!((keys_status(&server, "0012345").await, keys_status(&server, "12345").await), (200, 401));
}

// ─── S3: ambient AWS credentials ────────────────────────────────────────────

#[tokio::test]
async fn aws_credentials_in_the_environment_leave_local_storage_alone() {
    let dir = site(&[]);
    start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE").env("AWS_SECRET_ACCESS_KEY", "x").env("AWS_SESSION_TOKEN", "y");
        cmd
    })
    .await
    .up("AWS_* beside local storage");
}

/// A bucket that takes every request (wiremock on 127.0.0.1).
async fn recording_bucket() -> MockServer {
    let bucket = MockServer::start().await;
    Mock::given(wiremock::matchers::any()).respond_with(ResponseTemplate::new(200)).mount(&bucket).await;
    bucket
}

/// After one image generation, which stores its result in `bucket`: the key
/// id that signed each request `bucket` got (`Credential=<key id>/…`), or
/// `unsigned`.
async fn key_ids_signing_an_upload(server: &Server, bucket: &MockServer) -> Vec<String> {
    let response = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", server.base))
        .bearer_auth(MASTER)
        .json(&json!({ "model": "mock/image-gen", "prompt": "a red cube" }))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "the generation failed ({}):\n{}", response.status(), server.log());
    let requests = bucket.received_requests().await.unwrap_or_default();
    assert!(requests.iter().any(|r| r.method.as_str() == "PUT"), "nothing was stored in the bucket:\n{}", server.log());
    requests
        .iter()
        .map(|r| {
            let authorization = r.headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default();
            match authorization.split_once("Credential=") {
                Some((_, rest)) => rest.split('/').next().unwrap_or_default().to_string(),
                None => "unsigned".to_string(),
            }
        })
        .collect()
}

/// M1: the old example's blank keys, with `AWS_*` set beside them, leave the
/// AWS chain to sign, as a file with no key lines does (the first case, which
/// shows the chain's key reaches the bucket). Taken as the key, a blank key
/// signs every upload with an empty key id, which S3 refuses.
#[tokio::test]
async fn blank_s3_keys_in_the_file_leave_the_aws_chain_to_sign() {
    for (what, keys) in [("no key lines", ""), ("blank keys, as the old example wrote them", "    access_key_id: \"\"\n    secret_access_key: \"\"\n")] {
        let bucket = recording_bucket().await;
        let yaml = format!("image_storage:\n  backend: s3\n  s3:\n    bucket_name: bkt\n    endpoint_url: \"{}\"\n{keys}", bucket.uri());
        let dir = site(&[("litegen.yaml", &yaml)]);
        let server = start(dir.path(), |port| {
            let mut cmd = served(dir.path(), port);
            cmd.env("LITEGEN__PROVIDERS__MOCK__API_KEY", "")
                .env("AWS_ACCESS_KEY_ID", "AKIAE2ECHAIN")
                .env("AWS_SECRET_ACCESS_KEY", "e2e-chain-secret");
            cmd
        })
        .await
        .up(what);
        let ids = key_ids_signing_an_upload(&server, &bucket).await;
        assert!(ids.iter().all(|id| id == "AKIAE2ECHAIN"), "{what}: the bucket's requests were signed by {ids:?}, not the AWS chain's AKIAE2ECHAIN");
    }
}

// ─── S6: which of two settings wins ─────────────────────────────────────────

#[tokio::test]
async fn a_litegen_variable_beats_the_vendors_own() {
    let vendor = recording_vendor().await;
    let dir = site(&[]);
    let server = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("LITEGEN__PROVIDERS__OPENAI__API_BASE", format!("{}/v1", vendor.uri()))
            .env("LITEGEN__PROVIDERS__OPENAI__API_KEY", "sk-explicit")
            .env("OPENAI_API_KEY", "sk-bare");
        cmd
    })
    .await
    .up("both OpenAI keys");
    assert_eq!(key_sent(&server, &vendor).await, "Bearer sk-explicit");
}

#[tokio::test]
async fn an_empty_vendor_variable_does_not_blank_a_configured_key() {
    // Compose passes an unset `${OPENAI_API_KEY:-}` as "".
    let dir = site(&[("litegen.yaml", "providers:\n  openai:\n    api_key: sk-yaml\n")]);
    let server = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("OPENAI_API_KEY", "");
        cmd
    })
    .await
    .up("an empty OPENAI_API_KEY");
    let models: Value = reqwest::Client::new()
        .get(format!("{}/v1/models?media_type=image", server.base))
        .bearer_auth(MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let openai: Vec<&Value> = models["data"].as_array().unwrap().iter().filter(|m| m["provider"] == "openai").collect();
    assert!(!openai.is_empty(), "{models}");
    assert!(openai.iter().all(|m| m["is_available"] == true), "the file's key must still configure OpenAI: {openai:?}");
}

/// Guard: a vendor's variable still beats the file, which the example's
/// `api_key: ""  # or set OPENAI_API_KEY` relies on.
#[tokio::test]
async fn a_vendor_variable_beats_the_file() {
    let vendor = recording_vendor().await;
    let dir = site(&[("litegen.yaml", "providers:\n  openai:\n    api_key: sk-yaml\n")]);
    let server = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("LITEGEN__PROVIDERS__OPENAI__API_BASE", format!("{}/v1", vendor.uri())).env("OPENAI_API_KEY", "sk-bare");
        cmd
    })
    .await
    .up("the file's key and the vendor's variable");
    assert_eq!(key_sent(&server, &vendor).await, "Bearer sk-bare");
}

/// Guard: a `.env` file still configures litegen, now that it is loaded
/// before the runtime starts.
#[tokio::test]
async fn a_dot_env_file_still_configures_litegen() {
    let dir = site(&[]);
    let server = start(dir.path(), |port| {
        std::fs::write(
            dir.path().join(".env"),
            format!("LITEGEN__SERVER__HOST=127.0.0.1\nLITEGEN__SERVER__PORT={port}\nLITEGEN__MASTER_KEY={MASTER}\n"),
        )
        .unwrap();
        command(dir.path())
    })
    .await
    .up("a .env file");
    assert_eq!(keys_status(&server, MASTER).await, 200);
}

/// Guard: LITEGEN_ASSET_DIR beats the `LITEGEN__` form of the same setting,
/// so an image's `ENV LITEGEN__IMAGE_STORAGE__LOCAL_DIR` can't beat it.
#[tokio::test]
async fn litegen_asset_dir_beats_the_double_underscore_form() {
    let dir = site(&[]);
    let _server = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("LITEGEN__IMAGE_STORAGE__LOCAL_DIR", "a").env("LITEGEN_ASSET_DIR", "b");
        cmd
    })
    .await
    .up("both asset directories");
    assert!(dir.path().join("b").is_dir(), "LITEGEN_ASSET_DIR's directory was not used");
    assert!(!dir.path().join("a").exists(), "LITEGEN__IMAGE_STORAGE__LOCAL_DIR's directory was used");
}

// ─── S2: the database when no setting names one ─────────────────────────────

/// `served` with no database setting at all, so the built-in default applies.
fn on_the_default_database(dir: &Path, port: u16) -> Command {
    let mut cmd = served(dir, port);
    cmd.env_remove("LITEGEN__DATABASE_URL");
    cmd
}

/// The status of `POST /v1/keys` for a key named `name`.
async fn create_key(server: &Server, name: &str) -> u16 {
    let client = reqwest::Client::new();
    let response = client.post(format!("{}/v1/keys", server.base)).bearer_auth(MASTER).json(&json!({ "name": name })).send().await.unwrap();
    response.status().as_u16()
}

/// The names of the keys `GET /v1/keys` lists.
async fn key_names(server: &Server) -> Vec<String> {
    let client = reqwest::Client::new();
    let keys: Value = client.get(format!("{}/v1/keys", server.base)).bearer_auth(MASTER).send().await.unwrap().json().await.unwrap();
    keys["data"].as_array().unwrap_or_else(|| panic!("{keys}")).iter().filter_map(|k| k["name"].as_str().map(String::from)).collect()
}

#[tokio::test]
async fn a_first_start_with_no_database_setting_keeps_its_database_in_data() {
    // The Docker image's case too: /app is not writable there, /app/data is.
    let dir = site(&[]);
    let server = start(dir.path(), |port| on_the_default_database(dir.path(), port)).await.up("no database setting, in an empty directory");
    assert!(dir.path().join("data/litegen.db").is_file(), "the database is data/litegen.db");
    assert!(!dir.path().join("litegen.db").exists(), "and nothing is made beside it");
    assert_eq!(create_key(&server, "survives-restart").await, 201, "{}", server.log());
    drop(server);

    let server = start(dir.path(), |port| on_the_default_database(dir.path(), port)).await.up("the same directory again");
    assert!(key_names(&server).await.contains(&"survives-restart".to_string()), "the key is gone after a restart:\n{}", server.log());
}

#[tokio::test]
async fn a_litegen_db_left_by_the_old_default_is_still_used_and_named() {
    // The old default opened ./litegen.db, which a first start could not make:
    // whoever ran on it made the file. Moving the default must not leave it.
    let dir = site(&[]);
    let server = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env("LITEGEN__DATABASE_URL", "sqlite://litegen.db?mode=rwc");
        cmd
    })
    .await
    .up("on ./litegen.db");
    assert_eq!(create_key(&server, "from-before").await, 201, "{}", server.log());
    drop(server);

    let server = start(dir.path(), |port| on_the_default_database(dir.path(), port)).await.up("no database setting, beside ./litegen.db");
    let log = server.log();
    assert!(key_names(&server).await.contains(&"from-before".to_string()), "./litegen.db is no longer the database:\n{log}");
    assert!(!dir.path().join("data/litegen.db").exists(), "an empty data/litegen.db was made beside it");
    let names_both = |line: &str| line.contains("WARN") && line.contains("data/litegen.db") && line.matches("litegen.db").count() >= 2;
    assert!(log.lines().any(names_both), "no warning names litegen.db and data/litegen.db:\n{log}");
}

/// S2 fix-up M1: a URL meant for another database refuses to start, naming
/// only its scheme. sqlx reads such a URL as a SQLite file name, so with
/// `?mode=rwc`, the fix S2's errors offered, litegen made folders named after
/// its credentials and started on a new, empty SQLite database.
#[tokio::test]
async fn a_database_url_for_another_database_refuses_to_start_naming_only_its_scheme() {
    let dir = site(&[]);
    let (status, out) = start(dir.path(), |port| {
        let mut cmd = served(dir.path(), port);
        cmd.env_remove("LITEGEN__DATABASE_URL")
            .env("LITEGEN_DATABASE_URL", "postgresql+psycopg://u:s3cret-pw@db.example:5432/litegen?mode=rwc");
        cmd
    })
    .await
    .exited("a SQLAlchemy-style Postgres URL with ?mode=rwc");
    assert!(!status.success() && out.contains("`postgresql+psycopg`") && !out.contains("s3cret-pw"), "{status}: {out}");
    let made: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("postgresql+psycopg"))
        .collect();
    assert!(made.is_empty(), "made {made:?} in {}", dir.path().display());
}
