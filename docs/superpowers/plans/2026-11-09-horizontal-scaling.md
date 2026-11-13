# Horizontal Scaling with Redis: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** With a Redis URL configured, any number of litegen replicas behind a
plain load balancer share their rate limits, parallel slots, sign-in limit,
provider circuit breaker and image cache. Admin changes reach every replica,
each job is polled by one replica at a time, and file links verify on every
replica.

**Architecture:** A new server module, `litegen-core/src/shared/`, owns one
Redis client: lazy connect, a 500 ms timeout per call, and LiteLLM's circuit
breaker. Each per-replica piece gets an optional Redis backend and falls back
to its current local behavior when Redis fails. Fail-closed is an opt-in. The
engine (`proxy/circuit_breaker.rs`) gets a small trait, and the server
implements it with Redis. Config sync, poller claims and the link-signing key
go through the database, so they also work without Redis.

**Tech Stack:** Rust 1.99.1, tokio, the `redis` crate (redis-rs) with rustls,
Lua scripts, sqlx (Postgres and SQLite), axum.

**Spec:** `docs/superpowers/specs/2026-11-09-horizontal-scaling-design.md`. The
spec is approved. Read it before any task: it holds the reasons and the
decisions this plan carries out.

## Global Constraints

### Variable names and defaults (exact)

- **Redis:** `LITEGEN_REDIS_URL`, `LITEGEN_REDIS_HOST`, `LITEGEN_REDIS_PORT`
  (default `6379`), `LITEGEN_REDIS_PASSWORD`, `LITEGEN_REDIS_USERNAME`,
  `LITEGEN_REDIS_SSL` (`true` gives `rediss://`), `LITEGEN_REDIS_SSL_CA_CERTS`,
  and `LITEGEN_REDIS_SSL_CERT_REQS` (`required`, the default, or `none`). A URL
  wins over the parts.
- **Database:** every name exists as both `LITEGEN_DATABASE_<X>` and
  `LITEGEN_DB_<X>`, where `<X>` is one of `URL`, `HOST`, `PORT` (default
  `5432`), `USER` or `USERNAME`, `PASSWORD`, `NAME`, `SSLMODE` and
  `SSLROOTCERT`. The parts build `postgres://user:password@host:port/name?sslmode=…&sslrootcert=…`,
  with each part percent-encoded.
- **Database precedence:**
  1. `LITEGEN_DATABASE_URL` or `LITEGEN_DB_URL`;
  2. `LITEGEN__DATABASE_URL` (the old name, which keeps working);
  3. the parts;
  4. `database_url` in `litegen.yaml`;
  5. `sqlite://litegen.db`.
- **Outage settings:** `LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT` (default
  `false`), `LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD` (default `5`)
  and `LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT` (seconds, default
  `60`).

### Fixed copy

The 503 body, when fail-closed refuses a request, is `type` and `code`
`rate_limit_unverifiable`, sent with `Retry-After: 1`, and this message:
"Rate limit enforcement unavailable: request counters could not be verified
against Redis, and LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT is on, so the
request was refused to avoid exceeding the configured limit. Retry shortly."

### Values this plan pins

| Value | Setting |
|---|---|
| Key prefix | `litegen:`. Tests use `litegen-test:<uuid>:` |
| Redis call timeout | 500 ms |
| Slot lease | 30 s, refreshed every 10 s |
| Largest shared cache entry | 256 KiB (`MAX_SHARED_ENTRY_BYTES = 262_144`) |
| Poll claim hold | 30 s |
| Pub/sub channel | `<prefix>events` |
| Events | `{"event":"reload"|"cache-clear","from":"<replica id>"}` |

### Rules that hold for every task

- **Secrets:** no Redis or database password ever appears in a log line, an
  error, a `Debug` output or an API answer.
- **API:** no OpenAPI change: no route, field or header. If a task finds it
  needs one, stop and tell the user. The SDK regeneration handshake then
  applies.
- **Engine boundary** (`.claude/rules/engine-boundary.md`): `src/shared/` is
  server code. Engine files (`proxy/circuit_breaker.rs`, `capabilities/`)
  name nothing from the server, and `cargo test --lib engine_boundary`
  stays green.
- **The shared checkout** (`.claude/rules/working-in-this-repo.md`):
  - make no commits and leave nothing staged;
  - before the first edit of a task, send its file list to the sessions
    working in `litegen-core` (litegen-cd asked to hear);
  - before each cargo run, check that `pgrep -x cargo` prints nothing, send
    "starting" to litegen-cd, and send "done" afterwards;
  - run cargo from `litegen-core/` with
    `PATH="/opt/homebrew/opt/rustup/bin:$PATH"`, and add
    `CARGO_INCREMENTAL=0` when the disk is short (`df -h /`);
  - never use `--offline`. After adding a dependency, `git diff Cargo.lock`
    must show only additions.
- **Postgres SQL changes:** run the gated Postgres tests against a throwaway
  container (`.claude/rules/database-tests.md`). Pick a free port, for
  example 55439.
- **Redis tests** skip themselves unless `LITEGEN_REDIS_TEST_URL` is set. To
  run them:

  ```sh
  docker run -d --rm --name litegen-redis-test -p 127.0.0.1:56379:6379 redis:7-alpine
  export LITEGEN_REDIS_TEST_URL=redis://127.0.0.1:56379
  ```

  Stop the container with `docker stop litegen-redis-test` when the task is
  done. Every test uses its own key prefix, so the tests can run in parallel.
- **Ledger:** after each task, append to
  `.superpowers/sdd/2026-11-09-horizontal-scaling/ledger.md`:
  - the task;
  - the files touched;
  - the commands run and their results;
  - a time taken from `date`.

## Review Focus

These five conditions are implied by the spec, but no other test covers them.
Each one's test is added to the task named.

1. **A slow Redis, not a dead one.** A call that hangs returns within 600 ms
   and counts as a failure. Once the breaker opens, calls return at once. The
   test, `a_hanging_redis_costs_at_most_the_timeout`, is in Task 2.
2. **A burst across replicas in the same instant.** Fifty concurrent takes on
   one key with rpm 10, split over two limiters, allow exactly 10. The test,
   `a_burst_across_two_limiters_allows_exactly_the_limit`, is in Task 3.
3. **A Redis that is flushed or restarted.**
   - Scripts reload after `SCRIPT FLUSH`. Test
     `script_flush_is_survived`, Task 3.
   - Releasing a slot whose set is gone is not an error. Test
     `releasing_after_a_flush_is_quiet`, Task 4.
   - A missing cache epoch reads as 0. Test
     `a_missing_epoch_reads_as_zero`, Task 8.
4. **Clock skew between replicas.** Every script takes the time from
   `redis.call('TIME')`, never from the replica. Tests
   `scripts_use_the_redis_clock`, in Tasks 3, 4 and 6, assert each script's
   source contains `redis.call('TIME')` and no `ARGV` timestamp.
5. **Redis down at boot.** litegen starts, its calls fail fast, and they work
   once Redis is reachable. The test,
   `starts_with_redis_down_and_recovers`, is in Task 2.

---

### Task 1: Connection settings

**Files:**
- Create: `litegen-core/src/config/connections.rs`
- Modify: `litegen-core/src/config/mod.rs`:
  - `pub mod connections;`
  - in `AppConfig` (around line 52), add `#[serde(skip)] pub redis: Option<connections::RedisSettings>` and `#[serde(skip)] pub startup_warnings: Vec<String>`;
  - the `Default` impl (around line 180);
  - `load_config()` (around line 920).
- Modify: `litegen-core/src/main.rs`: log `config.startup_warnings` right after `init_tracing`.
- Create: `docs/scaling.md`, with a "Connection settings" section: both tables from the spec's Configuration, the precedence and the edge cases.
- Modify: `.env.deploy.template`: commented examples of the URL form and the parts form.
- Test: `litegen-core/src/config/connections.rs` (`#[cfg(test)] mod tests`).

**Interfaces:**
- Produces:
  - `pub struct Connections { pub database_url: String, pub redis: Option<RedisSettings>, pub warnings: Vec<String> }`
  - `pub fn resolve_connections(env: &HashMap<String, String>, configured_database_url: &str) -> Result<Connections, String>`. `configured_database_url` is what config-rs produced, which is the old variable, else `litegen.yaml`, else the default. `Err` holds the message that names the variables involved.
  - `#[derive(Clone)] pub struct RedisSettings` with fields `ca_certs: Option<PathBuf>`, `verify_certificates: bool`, `breaker_failure_threshold: u32`, `breaker_recovery: Duration`, `fail_closed: bool`, and a private `url: String`. Methods: `pub fn url(&self) -> &str`, `pub fn target(&self) -> String` (`host:port`, no credentials), `pub fn from_url(url: &str) -> RedisSettings` (the defaults: verify on, 5, 60 s, fail-open).
  - A hand-written `Debug` for `RedisSettings` that prints `target()` only.

- [ ] **Step 1: Write the failing tests**

```rust
fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> { pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
const DEFAULT: &str = "sqlite://litegen.db";

#[test] fn no_redis_variables_means_no_redis() {
    assert!(resolve_connections(&env(&[]), DEFAULT).unwrap().redis.is_none());
}
#[test] fn redis_parts_build_a_url_with_default_port() {
    let c = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "cache.internal")]), DEFAULT).unwrap();
    assert_eq!(c.redis.unwrap().url(), "redis://cache.internal:6379");
}
#[test] fn redis_parts_percent_encode_username_and_password() {
    let c = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_REDIS_PORT", "25061"),
        ("LITEGEN_REDIS_USERNAME", "default"), ("LITEGEN_REDIS_PASSWORD", "p@ss:w/rd%"), ("LITEGEN_REDIS_SSL", "true")]), DEFAULT).unwrap();
    assert_eq!(c.redis.unwrap().url(), "rediss://default:p%40ss%3Aw%2Frd%25@h:25061");
}
#[test] fn redis_url_wins_over_parts_and_warns() {
    let c = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "redis://a:6379"), ("LITEGEN_REDIS_HOST", "b")]), DEFAULT).unwrap();
    assert_eq!(c.redis.unwrap().url(), "redis://a:6379");
    assert!(c.warnings.iter().any(|w| w.contains("LITEGEN_REDIS_HOST") && w.contains("LITEGEN_REDIS_URL")));
}
#[test] fn redis_cert_reqs_none_turns_verification_off() {
    let c = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "rediss://a:1"), ("LITEGEN_REDIS_SSL_CERT_REQS", "none")]), DEFAULT).unwrap();
    assert!(!c.redis.unwrap().verify_certificates);
    assert!(resolve_connections(&env(&[("LITEGEN_REDIS_URL", "rediss://a:1"), ("LITEGEN_REDIS_SSL_CERT_REQS", "maybe")]), DEFAULT)
        .unwrap_err().contains("LITEGEN_REDIS_SSL_CERT_REQS"));
}
#[test] fn outage_settings_parse_with_defaults() {
    let r = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h")]), DEFAULT).unwrap().redis.unwrap();
    assert_eq!((r.breaker_failure_threshold, r.breaker_recovery, r.fail_closed), (5, Duration::from_secs(60), false));
    let r = resolve_connections(&env(&[("LITEGEN_REDIS_HOST", "h"), ("LITEGEN_FAIL_CLOSED_RATE_LIMIT_ENFORCEMENT", "true"),
        ("LITEGEN_REDIS_CIRCUIT_BREAKER_FAILURE_THRESHOLD", "3"), ("LITEGEN_REDIS_CIRCUIT_BREAKER_RECOVERY_TIMEOUT", "10")]), DEFAULT).unwrap().redis.unwrap();
    assert_eq!((r.breaker_failure_threshold, r.breaker_recovery, r.fail_closed), (3, Duration::from_secs(10), true));
}
#[test] fn database_url_new_names_win() {
    for name in ["LITEGEN_DATABASE_URL", "LITEGEN_DB_URL"] {
        let c = resolve_connections(&env(&[(name, "postgres://x/y"), ("LITEGEN__DATABASE_URL", "postgres://old/db")]), "postgres://old/db").unwrap();
        assert_eq!(c.database_url, "postgres://x/y");
        assert!(c.warnings.iter().any(|w| w.contains("LITEGEN__DATABASE_URL")));
    }
}
#[test] fn database_and_db_spellings_that_differ_refuse_to_start() {
    let e = resolve_connections(&env(&[("LITEGEN_DATABASE_HOST", "a"), ("LITEGEN_DB_HOST", "b")]), DEFAULT).unwrap_err();
    assert!(e.contains("LITEGEN_DATABASE_HOST") && e.contains("LITEGEN_DB_HOST"));
}
#[test] fn database_and_db_spellings_that_match_are_fine() {
    let c = resolve_connections(&env(&[("LITEGEN_DATABASE_URL", "postgres://x/y"), ("LITEGEN_DB_URL", "postgres://x/y")]), DEFAULT).unwrap();
    assert_eq!(c.database_url, "postgres://x/y");
}
#[test] fn legacy_double_underscore_url_beats_parts_and_warns() {
    let c = resolve_connections(&env(&[("LITEGEN__DATABASE_URL", "postgres://old/db"), ("LITEGEN_DB_HOST", "h"),
        ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")]), "postgres://old/db").unwrap();
    assert_eq!(c.database_url, "postgres://old/db");
    assert!(c.warnings.iter().any(|w| w.contains("LITEGEN_DB_HOST")));
}
#[test] fn database_parts_build_a_postgres_url() {
    let c = resolve_connections(&env(&[("LITEGEN_DB_HOST", "db.internal"), ("LITEGEN_DB_USERNAME", "lite gen"),
        ("LITEGEN_DATABASE_PASSWORD", "p@ss"), ("LITEGEN_DB_NAME", "litegen"), ("LITEGEN_DB_SSLMODE", "verify-full"),
        ("LITEGEN_DB_SSLROOTCERT", "/etc/ca.crt")]), DEFAULT).unwrap();
    assert_eq!(c.database_url, "postgres://lite%20gen:p%40ss@db.internal:5432/litegen?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.crt");
}
#[test] fn database_parts_missing_user_or_name_refuse_to_start() {
    assert!(resolve_connections(&env(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_NAME", "n")]), DEFAULT).unwrap_err().contains("LITEGEN_DATABASE_USER"));
    assert!(resolve_connections(&env(&[("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u")]), DEFAULT).unwrap_err().contains("LITEGEN_DATABASE_NAME"));
}
#[test] fn the_configured_url_is_kept_when_nothing_is_set() {
    assert_eq!(resolve_connections(&env(&[]), "sqlite://from-yaml.db").unwrap().database_url, "sqlite://from-yaml.db");
}
#[test] fn database_parts_beat_the_yaml_url_and_a_new_url_beats_parts() {
    let parts = [("LITEGEN_DB_HOST", "h"), ("LITEGEN_DB_USER", "u"), ("LITEGEN_DB_NAME", "n")];
    assert_eq!(resolve_connections(&env(&parts), "sqlite://from-yaml.db").unwrap().database_url, "postgres://u@h:5432/n", "no password: no colon");
    let mut with_url = parts.to_vec(); with_url.push(("LITEGEN_DATABASE_URL", "postgres://x/y"));
    let c = resolve_connections(&env(&with_url), DEFAULT).unwrap();
    assert_eq!(c.database_url, "postgres://x/y");
    assert!(c.warnings.iter().any(|w| w.contains("LITEGEN_DB_HOST") && w.contains("LITEGEN_DATABASE_URL")));
}
#[test] fn redis_ca_certs_path_is_kept() {
    let r = resolve_connections(&env(&[("LITEGEN_REDIS_URL", "rediss://a:1"), ("LITEGEN_REDIS_SSL_CA_CERTS", "/etc/redis-ca.pem")]), DEFAULT).unwrap().redis.unwrap();
    assert_eq!(r.ca_certs.as_deref(), Some(std::path::Path::new("/etc/redis-ca.pem")));
    assert!(r.verify_certificates, "checks stay on by default");
}
#[test] fn debug_output_never_shows_a_password() {
    let r = RedisSettings::from_url("redis://u:hunter2@h:6379");
    assert!(!format!("{r:?}").contains("hunter2"));
    let e = resolve_connections(&env(&[("LITEGEN_DATABASE_PASSWORD", "hunter2"), ("LITEGEN_DB_PASSWORD", "other"), ("LITEGEN_DB_HOST", "h")]), DEFAULT).unwrap_err();
    assert!(!e.contains("hunter2") && !e.contains("other"));
}
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test --lib config::connections`
Expected: compile failure, because `resolve_connections` is not defined.

- [ ] **Step 3: Implement `resolve_connections` and `RedisSettings`** in `connections.rs`.
  - Percent-encode the user, password, database name and query values with the `percent-encoding` crate. Use `NON_ALPHANUMERIC` minus RFC 3986's unreserved marks `-`, `.`, `_` and `~`, so that `/etc/ca.crt` becomes `%2Fetc%2Fca.crt`. Hosts are not encoded. Add the crate to `Cargo.toml` if it isn't a direct dependency already.
  - Error and warning texts name the variables and never their values.
  - In `load_config()`, after `try_deserialize()`, call
    `resolve_connections(&std::env::vars().collect(), &config.database_url)`.
    Assign its three outputs, and map `Err(msg)` to
    `config::ConfigError::Message(msg)`. `main` already prints that and
    exits 1.

- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --lib config::`
Expected: PASS, including the existing config tests.

- [ ] **Step 5: Write the docs**

Write `docs/scaling.md`, "Connection settings": both tables, the precedence
list and the edge cases, as in the spec's Configuration. Add the examples to
`.env.deploy.template`.

- [ ] **Step 6: Ledger entry**

---

### Task 2: The shared Redis client

**Files:**
- Modify: `litegen-core/Cargo.toml`. Add `redis` at the latest release on crates.io (`cargo search redis --limit 1`), with `default-features = false`. Enable the features that version names for: the tokio runtime, the connection manager, rustls TLS with its tokio support, and scripts. sqlx and rust-s3 already use rustls. `Cargo.lock` gains additions only.
- Create: `litegen-core/src/shared/mod.rs` (`SharedRedis`, `SharedError`, `replica_id`, `startup_notes`)
- Create: `litegen-core/src/shared/breaker.rs` (`RedisBreaker`)
- Create: `litegen-core/src/shared/testing.rs` (`#[doc(hidden)] pub`, `CutProxy`, test-only)
- Modify: `litegen-core/src/lib.rs` (`pub mod shared;`)
- Modify: `litegen-core/src/proxy/router.rs`:
  - add the field `shared: Option<Arc<SharedRedis>>`, set to `None` in `new`;
  - add `pub fn with_shared(mut self, shared: Arc<SharedRedis>) -> Self`, which only stores it for now;
  - add `pub fn shared(&self) -> Option<&Arc<SharedRedis>>`.
- Modify: `litegen-core/src/main.rs`:
  - after the DB connects, when `config.redis` is set, call `SharedRedis::connect(redis, "litegen:")`;
  - build the router with `ProxyRouter::new(..).with_shared(s)` when there is one;
  - log `startup_notes(&config)`.

**Interfaces:**
- Consumes: `RedisSettings` (Task 1).
- Produces:
  - `pub async fn SharedRedis::connect(settings: &RedisSettings, key_prefix: &str) -> Result<Arc<SharedRedis>, SharedError>`. It fails only on bad settings (an unparsable URL, an unreadable CA file), never because Redis is unreachable. The connection is made lazily, and again after a drop.
  - `pub fn key(&self, parts: &[&str]) -> String`: the prefix plus the parts joined with `:`.
  - `pub fn fail_closed(&self) -> bool`
  - `pub fn replica_id() -> &'static str`: a UUID made once per process.
  - `pub async fn run<T, F, Fut>(&self, op: F) -> Result<T, SharedError>`, where `F: FnOnce(redis::aio::ConnectionManager) -> Fut` and `Fut: Future<Output = redis::RedisResult<T>>`. When the breaker is open, it returns `Err(SharedError::Unavailable)` without touching the network. Otherwise it runs `op` under the 500 ms timeout and records the result on the breaker.
  - `pub async fn publish(&self, channel: &str, message: &str) -> Result<(), SharedError>`
  - `pub async fn subscribe(&self, channel: &str) -> Result<redis::aio::PubSub, SharedError>`
  - `pub enum SharedError { Unavailable, Redis(String) }`. The `String` never holds credentials.
  - `RedisBreaker::new(threshold: u32, recovery: Duration)`, `allow(&self, now: Instant) -> bool` and `record(&self, ok: bool, now: Instant)`. They follow LiteLLM: the breaker opens after `threshold` consecutive failures, a single probe goes out after `recovery`, a successful probe closes it, and a failed probe opens it again.
  - `pub struct StartupNote { pub warn: bool, pub text: String }` and `pub fn startup_notes(config: &AppConfig) -> Vec<StartupNote>`.
  - `shared::testing` is `#[doc(hidden)] pub`. It's always compiled, so the lib tests and the integration tests share it, and production never calls it. It holds:
    - `CutProxy::start(upstream: SocketAddr) -> CutProxy`, a tokio TCP forwarder, with `addr()`, `url() -> String`, `cut()`, `hang()` and `restore()`. `cut` drops the connections and refuses new ones. `hang` accepts connections and never forwards.
    - `test_url() -> Option<String>`, which reads `LITEGEN_REDIS_TEST_URL` and prints a skip line when it's unset.
    - `test_prefix() -> String`, which returns `litegen-test:<uuid>:`.
    - `addr_of(url: &str) -> SocketAddr`
    - `two_shared() -> Option<(Arc<SharedRedis>, Arc<SharedRedis>)>`: two clients on one fresh prefix, playing two replicas.
    - `dead_redis(fail_closed: bool) -> Arc<SharedRedis>`, which points at `redis://127.0.0.1:1` and needs no container.
    - `flush_scripts(&SharedRedis)` (`SCRIPT FLUSH`) and `flush_keys(&SharedRedis)`, which deletes this prefix's keys.
  - At boot, `main` sends one `PING` and logs `Redis at <target>: reachable`, or `unreachable; litegen keeps running and retries`, naming `target()` only.
- TLS: with `verify_certificates == false`, use the URL's `#insecure` form. With `ca_certs`, load the PEM and connect through the crate's custom root-certificate builder.

- [ ] **Step 1: Write the failing tests** (`shared/breaker.rs` and `shared/mod.rs`)

```rust
#[test] fn breaker_opens_after_threshold_and_probes_after_recovery() {
    let b = RedisBreaker::new(3, Duration::from_secs(60)); let t0 = Instant::now();
    for _ in 0..3 { assert!(b.allow(t0)); b.record(false, t0); }
    assert!(!b.allow(t0 + Duration::from_secs(59)));
    assert!(b.allow(t0 + Duration::from_secs(60)), "one probe");
    assert!(!b.allow(t0 + Duration::from_secs(60)), "only one");
    b.record(false, t0 + Duration::from_secs(60));
    assert!(!b.allow(t0 + Duration::from_secs(119)), "a failed probe opens it again");
    assert!(b.allow(t0 + Duration::from_secs(120))); b.record(true, t0 + Duration::from_secs(120));
    assert!(b.allow(t0 + Duration::from_secs(120)), "closed");
}
#[test] fn notes_warn_on_sqlite_or_local_storage_with_redis_and_inform_without() {
    let mut c = AppConfig::default();
    assert_eq!(startup_notes(&c).iter().filter(|n| !n.warn).count(), 1, "the per-process info line");
    c.redis = Some(RedisSettings::from_url("redis://h:6379"));
    let notes = startup_notes(&c);
    assert!(notes.iter().any(|n| n.warn && n.text.contains("SQLite")));
    assert!(notes.iter().any(|n| n.warn && n.text.contains("local")));
}
#[tokio::test] async fn starts_with_redis_down_and_recovers() {
    let Some(url) = test_url() else { return };
    let proxy = CutProxy::start(addr_of(&url)).await; proxy.cut();
    let mut settings = RedisSettings::from_url(&proxy.url()); settings.breaker_recovery = Duration::from_secs(1);
    let s = SharedRedis::connect(&settings, &test_prefix()).await.expect("connects lazily");
    for _ in 0..5 { assert!(s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await.is_err()); }
    proxy.restore(); tokio::time::sleep(Duration::from_millis(1100)).await; // past the breaker's recovery
    assert_eq!(s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await.unwrap(), "PONG");
}
#[tokio::test] async fn a_hanging_redis_costs_at_most_the_timeout() {
    let Some(url) = test_url() else { return };
    let proxy = CutProxy::start(addr_of(&url)).await; proxy.hang();
    let s = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
    let t = Instant::now(); let _ = s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await;
    assert!(t.elapsed() < Duration::from_millis(600));
    for _ in 0..5 { let _ = s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await; }
    let t = Instant::now(); assert!(matches!(s.run(|mut c| async move { redis::cmd("PING").query_async::<String>(&mut c).await }).await, Err(SharedError::Unavailable)));
    assert!(t.elapsed() < Duration::from_millis(10), "open: no network");
}
```

The helpers (`test_url`, `test_prefix`, `addr_of`) come from
`shared::testing`.

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test --lib shared::`
Expected: compile failure.

- [ ] **Step 3: Implement** `SharedRedis`, `RedisBreaker`, `CutProxy`, `startup_notes`, the router accessors and the `main.rs` wiring, as listed under Interfaces.

- [ ] **Step 4: Run the tests and confirm they pass**

Run (with the Redis container up and `LITEGEN_REDIS_TEST_URL` set):
`cargo test --lib shared:: && cargo test --lib engine_boundary`
Expected: PASS. Without the variable set, the Redis tests print a skip line
and pass.

- [ ] **Step 5: Check the lock file.** `git diff Cargo.lock` must show only added packages.
- [ ] **Step 6: Ledger entry**

---

### Task 3: Shared rate limits

**Files:**
- Create: `litegen-core/src/shared/rate_limit.rs`
- Modify: `litegen-core/src/api/middleware/rate_limit.rs`:
  - give `RateLimiter` an optional `shared` field, and add `with_shared`;
  - add `LimitRefusal` and `unverifiable_response()`.
- Modify: `litegen-core/src/api/middleware/mod.rs:617` (`run_rate_limited`)
- Modify: `litegen-core/src/management/enforce.rs:457-468`
- Modify: `litegen-core/src/main.rs`: when Redis is set, build `rate_limiter` with `RateLimiter::with_shared(s.clone())`.
- Test: the existing tests in `rate_limit.rs`, plus new Redis-gated tests there, plus `auth_tests.rs` for the 503.

**Interfaces:**
- Consumes: `SharedRedis::run`, `SharedRedis::key` and `SharedRedis::fail_closed` (Task 2).
- Produces:
  - `pub enum LimitRefusal { Exceeded { retry_after_secs: u64 }, Unverifiable }`
  - `pub async fn RateLimiter::try_take(&self, key_id: Uuid, rpm: u32) -> Result<(), LimitRefusal>`. Only the error type changes.
  - `pub async fn snapshot(&self, key_id: Uuid, rpm: u32) -> RateLimitSnapshot`, with the same signature as today.
  - `pub fn RateLimiter::with_shared(shared: Arc<SharedRedis>) -> RateLimiter`
  - `pub fn shared(&self) -> Option<&Arc<SharedRedis>>`
  - `pub fn unverifiable_response() -> axum::response::Response`: the fixed 503 copy.
  - `shared::rate_limit::take(shared: &SharedRedis, bucket: &str, rpm: u32) -> Result<(bool, f64), SharedError>`, which returns whether the take was allowed and the tokens left.
  - `shared::rate_limit::peek(shared, bucket, rpm) -> Result<f64, SharedError>`
- The script has the same token-bucket math as `RateLimiter::try_take` today (capacity `rpm`, refill `rpm/60` per second, starting full). It keeps a hash per bucket under `key(["rl", bucket])`, with fields `tokens` and `ts_ms` and a `PEXPIRE` of 120000. The clock is `redis.call('TIME')`. Return the tokens as a string, because Redis truncates Lua numbers to integers. Run it through `redis::Script`, which reloads on `NOSCRIPT`.
- **Fallback:** on `SharedError`, if `fail_closed()`, return `Unverifiable`. Otherwise use the local buckets, and log one warning per minute at most. The local buckets are today's map, kept inside the limiter.
- **Callers:** `Exceeded` keeps today's 429 and its headers. `Unverifiable` answers `unverifiable_response()`. Both callers, `run_rate_limited` and `enforce::begin`, handle both cases.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn two_limiters_share_one_bucket() {
    let Some((a, b)) = two_shared_limiters().await else { return }; let k = Uuid::new_v4();
    assert!(a.try_take(k, 3).await.is_ok()); assert!(b.try_take(k, 3).await.is_ok()); assert!(a.try_take(k, 3).await.is_ok());
    assert!(matches!(b.try_take(k, 3).await, Err(LimitRefusal::Exceeded { .. })));
    assert_eq!(a.snapshot(k, 3).await.remaining, 0);
}
#[tokio::test] async fn a_burst_across_two_limiters_allows_exactly_the_limit() {
    let Some((a, b)) = two_shared_limiters().await else { return }; let k = Uuid::new_v4();
    let tries = (0..50).map(|i| { let l = if i % 2 == 0 { a.clone() } else { b.clone() }; async move { l.try_take(k, 10).await.is_ok() } });
    assert_eq!(futures::future::join_all(tries).await.into_iter().filter(|ok| *ok).count(), 10);
}
#[tokio::test] async fn script_flush_is_survived() {
    let Some((a, _)) = two_shared_limiters().await else { return };
    flush_scripts().await; assert!(a.try_take(Uuid::new_v4(), 5).await.is_ok());
}
#[tokio::test] async fn redis_down_falls_back_to_local() {
    let l = RateLimiter::with_shared(dead_redis(false).await); let k = Uuid::new_v4();
    assert!(l.try_take(k, 2).await.is_ok()); assert!(l.try_take(k, 2).await.is_ok());
    assert!(matches!(l.try_take(k, 2).await, Err(LimitRefusal::Exceeded { .. })));
}
#[tokio::test] async fn fail_closed_refuses_when_redis_is_down() {
    let l = RateLimiter::with_shared(dead_redis(true).await);
    assert!(matches!(l.try_take(Uuid::new_v4(), 2).await, Err(LimitRefusal::Unverifiable)));
}
#[test] fn scripts_use_the_redis_clock() {
    assert!(crate::shared::rate_limit::SCRIPT_SOURCE.contains("redis.call('TIME')"));
}
```

`two_shared_limiters()` maps `shared::testing::two_shared()` to
`(Arc<RateLimiter>, Arc<RateLimiter>)` through `RateLimiter::with_shared`.
`dead_redis(fail_closed)` connects to `redis://127.0.0.1:1`, so it needs no
container. Add one API test to `auth_tests.rs`,
`fail_closed_answers_503_rate_limit_unverifiable`: a key with an
`rpm_limit`, `AppState.rate_limiter = RateLimiter::with_shared(dead_redis(true))`.
Assert status 503, `retry-after: 1` and `error.code == "rate_limit_unverifiable"`.

- [ ] **Step 2: Run the tests and confirm they fail.** Run `cargo test --lib rate_limit` and `cargo test --lib auth_tests`. Expected: compile failure.
- [ ] **Step 3: Implement** the script module, the limiter's backend choice, `LimitRefusal` at both callers, and the `main.rs` line.
- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --lib rate_limit auth_tests enforce` with Redis up.
Expected: PASS. The existing `exhausted_rpm_returns_429_with_ratelimit_headers`
still passes unchanged.

- [ ] **Step 5: Ledger entry**

---

### Task 4: Shared parallel slots

**Files:**
- Create: `litegen-core/src/shared/slots.rs`
- Modify: `litegen-core/src/management/enforce.rs`:
  - add the `SlotGuard` enum, `acquire_slot` and the shared lease refresher;
  - change `LimitGuard._slots` to `Vec<SlotGuard>`;
  - change the acquire call at lines 445-455.
- Test: `enforce.rs` tests, plus Redis-gated tests in `shared/slots.rs`.

**Interfaces:**
- Consumes: `RateLimiter::shared()`, `LimitRefusal` and `unverifiable_response()` (Task 3).
- Produces:
  - `pub enum SlotGuard { Local(ParallelSlot), Shared(SharedSlot) }`
  - `pub async fn acquire_slot(limiter: &RateLimiter, entity_type: EntityType, entity_id: &str, max: u32) -> Result<SlotGuard, LimitRefusal>`. `ParallelSlot::try_acquire` and `in_flight()` stay as they are, as the local backend.
  - `shared::slots::acquire(shared, set_key: &str, member: &str, max: u32, lease: Duration) -> Result<bool, SharedError>`
  - `shared::slots::refresh(shared, set_key, member, lease) -> Result<(), SharedError>`
  - `shared::slots::release(shared, set_key, member) -> Result<(), SharedError>`
  - `pub const SLOT_LEASE: Duration = 30 s` and `pub const SLOT_REFRESH_EVERY: Duration = 10 s`.
- **The set:** `key(["slots", entity_type.as_str(), entity_id])` is a sorted set. A member is a fresh UUID, and its score is when the lease ends, in ms from `redis.call('TIME')`.
  - **Acquire** removes the scores already past, counts what's left, and if the count is under `max`, adds the member with score now plus the lease, and sets `PEXPIRE` to twice the lease.
  - **Refresh** is `ZADD XX`.
  - **Release** is `ZREM`.
- **`SharedSlot`** holds a refresher task. Its `Drop` aborts the refresher and spawns the release. A release that fails only logs.
- **Fallback:** the same as Task 3. If acquire fails, then `Unverifiable` when fail-closed, else a local slot.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn a_cap_holds_across_two_limiters() {
    let Some((a, b)) = two_shared_limiters().await else { return }; let id = Uuid::new_v4().to_string();
    let _s1 = acquire_slot(&a, EntityType::Key, &id, 2).await.unwrap();
    let _s2 = acquire_slot(&b, EntityType::Key, &id, 2).await.unwrap();
    assert!(acquire_slot(&a, EntityType::Key, &id, 2).await.is_err());
    drop(_s1); tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(acquire_slot(&b, EntityType::Key, &id, 2).await.is_ok(), "released");
}
#[tokio::test] async fn a_lost_holder_frees_its_slot_after_the_lease() {
    let Some((s, _)) = two_shared().await else { return }; let set = s.key(&["slots", "key", "t"]);
    assert!(acquire(&s, &set, "gone", 1, Duration::from_millis(300)).await.unwrap()); // never released or refreshed
    assert!(!acquire(&s, &set, "next", 1, Duration::from_millis(300)).await.unwrap());
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(acquire(&s, &set, "next", 1, Duration::from_millis(300)).await.unwrap());
}
#[tokio::test] async fn releasing_after_a_flush_is_quiet() {
    let Some((s, _)) = two_shared().await else { return }; let set = s.key(&["slots", "key", "f"]);
    acquire(&s, &set, "m", 1, SLOT_LEASE).await.unwrap(); flush_keys(&s).await;
    assert!(release(&s, &set, "m").await.is_ok());
}
#[test] fn scripts_use_the_redis_clock() { assert!(crate::shared::slots::SCRIPT_SOURCE.contains("redis.call('TIME')")); }
```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `cargo test --lib slots enforce`. Expected: compile failure.
- [ ] **Step 3: Implement** as listed under Interfaces. `enforce::begin` maps `Err(Exceeded)` to today's `parallel_limit_exceeded` 429, and `Err(Unverifiable)` to `unverifiable_response()`.
- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --lib slots enforce management::` with Redis up.
Expected: PASS. The existing test at `management/tests.rs:513`, which uses
the local backend, passes unchanged.

- [ ] **Step 5: Ledger entry**

---

### Task 5: The shared MCP sign-in limit

**Files:**
- Modify: `litegen-core/src/mcp/oauth/limit.rs`:
  - give `AddressLimiter` an optional shared backend;
  - make `try_take` async, returning `LimitRefusal`;
  - update the caller at line 114, where `Unverifiable` answers `unverifiable_response()`.
- Modify: `litegen-core/src/mcp/oauth/mod.rs:54`: `limiter: state.router.shared().map_or_else(AddressLimiter::default, |s| AddressLimiter::with_shared(s.clone()))`.
- Modify: `litegen-core/src/mcp/oauth/tests.rs:168`: await the call and match `LimitRefusal`.

**Interfaces:**
- Consumes: `shared::rate_limit::take` and `LimitRefusal` (Task 3).
- Produces:
  - `pub fn AddressLimiter::with_shared(shared: Arc<SharedRedis>) -> AddressLimiter`
  - `pub async fn try_take(&self, address: IpAddr, per_minute: u32, now: Instant) -> Result<(), LimitRefusal>`. The bucket is `"signin:" + address`.
- Falls back exactly as Task 3 does.

- [ ] **Step 1: Write the failing tests** (in `mcp/oauth/tests.rs`)

```rust
#[tokio::test] async fn two_limiters_share_an_address() {
    let Some((sa, sb)) = crate::shared::testing::two_shared().await else { return };
    let (a, b) = (AddressLimiter::with_shared(sa), AddressLimiter::with_shared(sb)); let ip: IpAddr = "203.0.113.9".parse().unwrap();
    assert!(a.try_take(ip, 2, Instant::now()).await.is_ok()); assert!(b.try_take(ip, 2, Instant::now()).await.is_ok());
    assert!(matches!(a.try_take(ip, 2, Instant::now()).await, Err(LimitRefusal::Exceeded { .. })));
}
#[tokio::test] async fn a_dead_redis_falls_back_to_this_replicas_count() {
    let l = AddressLimiter::with_shared(crate::shared::testing::dead_redis(false).await); let ip: IpAddr = "203.0.113.10".parse().unwrap();
    assert!(l.try_take(ip, 1, Instant::now()).await.is_ok());
    assert!(matches!(l.try_take(ip, 1, Instant::now()).await, Err(LimitRefusal::Exceeded { .. })));
}
```

- [ ] **Step 2: Run the tests and confirm they fail.** Run `cargo test --lib mcp::oauth`. Expected: compile failure.
- [ ] **Step 3: Implement** as listed under Interfaces.
- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --lib mcp::oauth` with Redis up, then `cargo test --test mcp_oauth_api`.
Expected: PASS.

- [ ] **Step 5: Ledger entry**

---

### Task 6: The provider circuit breaker: the re-open fix, then sharing

**Files:**
- Modify: `litegen-core/src/proxy/circuit_breaker.rs`. This is an engine file:
  - add `#[async_trait] pub trait BreakerStore`;
  - add `pub struct LocalBreakerStore`, which takes over today's map;
  - add `CircuitBreaker::with_store`;
  - fix `record_failure` in the local store.
- Modify: `litegen-core/src/proxy/circuit_breaker_tests.rs`
- Create: `litegen-core/src/shared/provider_breaker.rs` (`RedisBreakerStore`)
- Modify: `litegen-core/src/proxy/router.rs`: in `with_shared`, `self.circuit_breaker = Arc::new(CircuitBreaker::with_store(threshold, open_for, Arc::new(RedisBreakerStore::new(shared.clone()))))`, keeping the current threshold and open time.

**Interfaces:**
- Produces, in the engine:
  - `trait BreakerStore: Send + Sync`, with `async fn is_open(&self, provider: &str, open_for: Duration) -> bool`, `async fn record_success(&self, provider: &str)` and `async fn record_failure(&self, provider: &str, threshold: u32, open_for: Duration)`.
  - `CircuitBreaker::new(threshold, open_for)` is now `with_store(threshold, open_for, Arc::new(LocalBreakerStore::default()))`.
  - The public methods of `CircuitBreaker` keep their signatures: `is_open`, `record_success`, `record_failure`, `reconfigure`.
- Produces, in the server: `RedisBreakerStore::new(shared: Arc<SharedRedis>) -> RedisBreakerStore`.
  - The state is a hash per provider under `key(["breaker", provider])`, with fields `failures` and `opened_ms`, and a `PEXPIRE` of 86400000.
  - The `record_failure` script runs `HINCRBY failures 1`. When `failures >= threshold` and `opened_ms` is either missing or at least `open_for` old, it sets `opened_ms` to now, taken from `redis.call('TIME')`.
  - `record_success` deletes the hash.
  - `is_open` means `opened_ms` is present and now is before `opened_ms + open_for`.
  - On a `SharedError`, it uses an inner `LocalBreakerStore`.
- **The fix:** a failure while the breaker is half-open opens it again. In the local store the condition becomes `consecutive_failures >= threshold && opened_at.map_or(true, |t| t.elapsed() >= open_for)`. A failure that arrives while the breaker is open doesn't extend the window.

- [ ] **Step 1: Write the failing test for the bug** (in `circuit_breaker_tests.rs`)

```rust
#[tokio::test]
async fn a_failed_half_open_probe_opens_the_breaker_again() {
    let cb = CircuitBreaker::new(2, Duration::from_millis(100));
    cb.record_failure("p").await; cb.record_failure("p").await; assert!(cb.is_open("p").await);
    tokio::time::sleep(Duration::from_millis(120)).await; assert!(!cb.is_open("p").await, "half-open");
    cb.record_failure("p").await; assert!(cb.is_open("p").await, "the failed probe opens it again");
}
#[tokio::test]
async fn a_failure_while_open_does_not_extend_the_window() {
    let cb = CircuitBreaker::new(1, Duration::from_millis(100));
    cb.record_failure("p").await; tokio::time::sleep(Duration::from_millis(60)).await; cb.record_failure("p").await;
    tokio::time::sleep(Duration::from_millis(50)).await; assert!(!cb.is_open("p").await, "the first window ended on time");
}
```

- [ ] **Step 2: Run the tests and confirm the first one fails**

Run: `cargo test --lib circuit_breaker`
Expected: `a_failed_half_open_probe_opens_the_breaker_again` FAILS at "the
failed probe opens it again". The rest pass.

- [ ] **Step 3: Fix `record_failure`** as described above.
- [ ] **Step 4: Write the failing sharing tests** (in `shared/provider_breaker.rs`)

```rust
#[tokio::test] async fn two_breakers_share_state() {
    let Some((sa, sb)) = crate::shared::testing::two_shared().await else { return };
    let a = CircuitBreaker::with_store(2, Duration::from_secs(30), Arc::new(RedisBreakerStore::new(sa)));
    let b = CircuitBreaker::with_store(2, Duration::from_secs(30), Arc::new(RedisBreakerStore::new(sb)));
    a.record_failure("v").await; b.record_failure("v").await; assert!(a.is_open("v").await && b.is_open("v").await);
    b.record_success("v").await; assert!(!a.is_open("v").await);
}
#[tokio::test] async fn redis_breaker_reopens_after_a_failed_probe() { /* the same steps as the local test above, over RedisBreakerStore */ }
#[test] fn scripts_use_the_redis_clock() { assert!(SCRIPT_SOURCE.contains("redis.call('TIME')")); }
```

- [ ] **Step 5: Implement** the trait, `LocalBreakerStore`, `RedisBreakerStore` and the router wiring.
- [ ] **Step 6: Run the tests and confirm they pass**

Run: `cargo test --lib circuit_breaker provider_breaker engine_boundary router` with Redis up.
Expected: PASS. The engine boundary test confirms that `circuit_breaker.rs`
names nothing from the server.

- [ ] **Step 7: Ledger entry**

---

### Task 7: Config sync: the wider refresh and pub/sub

**Files:**
- Create: `litegen-core/src/shared/events.rs`
- Modify: `litegen-core/src/management/runtime.rs`:
  - widen `refresh`;
  - make `apply_at_boot` record versions;
  - add `spawn_runtime_sync`, which replaces `spawn_settings_refresher`;
  - add `announce`.
- Modify: `litegen-core/src/proxy/router.rs`: add the field `applied_runtime: std::sync::Mutex<AppliedRuntime>`, set to its default in `new`, and `pub(crate) fn applied_runtime(&self)`.
- Modify: `litegen-core/src/capabilities/registry.rs`: add `pub fn disabled_ids(&self) -> Vec<String>`. This is an engine file, but the method names nothing outside it.
- Modify: `litegen-core/src/api/handlers/management/admin.rs`. After each successful save, call `announce(&state, SharedEvent::Reload).await`, in:
  - `create_model`, `replace_model`, `patch_model` and `delete_model`;
  - `put_provider` and `delete_provider`;
  - `put_routes` and `delete_routes`;
  - `put_settings`.
- Modify: `litegen-core/src/main.rs:224`: `spawn_runtime_sync(state.clone(), Duration::from_secs(15), true)`.
- Modify: `docs/gateway-settings.md:148-154`. The "Several replicas" section now covers all five kinds of change: within about a second with Redis, and within 15 s without it.
- Test: `litegen-core/tests/gateway_settings_api.rs`, next to `a_setting_saved_on_one_instance_reaches_another_after_a_refresh` (line 208).

**Interfaces:**
- Consumes: `SharedRedis::publish`, `subscribe` and `replica_id` (Task 2), and `ProxyRouter::shared()`.
- Produces:
  - `pub enum SharedEvent { Reload, CacheClear }`
  - `pub async fn publish_event(shared: &SharedRedis, event: SharedEvent)`. It's best-effort and logs on failure.
  - `pub fn spawn_event_listener(shared: Arc<SharedRedis>, on_event: impl Fn(SharedEvent) + Send + Sync + 'static) -> tokio::task::JoinHandle<()>`. It resubscribes after a drop, backing off from 1 s to 30 s, and skips messages whose `from` is `replica_id()`.
  - `pub async fn refresh(state: &AppState) -> Result<bool, sqlx::Error>`. The signature stays, and it now also syncs runtime models, model states and provider configs:
    - a runtime model whose `updated_at` differs from `AppliedRuntime.models` is re-validated and upserted, as at boot;
    - a model recorded there but missing from the database is removed from the runtime layer;
    - model states are diffed against `registry.disabled_ids()`;
    - a provider whose row is new, changed or deleted goes through `apply_provider`.
    - It returns whether anything changed.
  - `pub(crate) struct AppliedRuntime { pub models: HashMap<String, DateTime<Utc>>, pub providers: HashMap<String, DateTime<Utc>> }`. `apply_at_boot` fills it. `BootReport` doesn't change, because `management/tests.rs:1085` compares it field by field.
  - `pub fn spawn_runtime_sync(state: Arc<AppState>, every: Duration, subscribe: bool) -> tokio::task::JoinHandle<()>`. It loops on `select!` over the 15 s tick and a `tokio::sync::Notify`, and calls `refresh` each time. With `subscribe` and a shared Redis, `spawn_event_listener` notifies the loop on `Reload`. Task 8 adds `CacheClear`. `subscribe = false` is the switch for tests the spec calls for, and no user can set it.
  - `pub async fn announce(state: &AppState, event: SharedEvent)`. It publishes when `state.router.shared()` is set, and otherwise does nothing.

- [ ] **Step 1: Write the failing tests** (SQLite file, two states, no Redis, following the pattern at line 208)

```rust
#[tokio::test] async fn a_runtime_model_saved_on_one_instance_reaches_another_after_a_refresh() {
    let (a, _, b, b_state) = two_instances().await;
    let (s, _, body) = send(&a, "POST", "/v1/admin/models", &[], Some(runtime_model_json("custom/one"))).await; assert_eq!(s, StatusCode::CREATED, "{body}");
    assert_eq!(send(&b, "GET", "/v1/models/custom/one", &[], None).await.0, StatusCode::NOT_FOUND, "B hasn't looked");
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    assert_eq!(send(&b, "GET", "/v1/models/custom/one", &[], None).await.0, StatusCode::OK);
    assert!(!litegen::management::runtime::refresh(&b_state).await.unwrap(), "nothing new the second time");
    send(&a, "DELETE", "/v1/admin/models/custom/one", &[], None).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    assert_eq!(send(&b, "GET", "/v1/models/custom/one", &[], None).await.0, StatusCode::NOT_FOUND, "gone from B too");
}
#[tokio::test] async fn a_model_disabled_on_one_instance_is_refused_on_another() {
    let (a, _, b, b_state) = two_instances().await;
    send(&a, "PATCH", "/v1/admin/models/mock/image-gen", &[], Some(json!({ "disabled": true }))).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    let (s, _, _) = send(&b, "POST", "/v1/images/generations", &[], Some(json!({ "model": "mock/image-gen", "prompt": "x" }))).await;
    assert_ne!(s, StatusCode::OK, "disabled on B");
    send(&a, "PATCH", "/v1/admin/models/mock/image-gen", &[], Some(json!({ "disabled": false }))).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
}
#[tokio::test] async fn a_provider_config_saved_on_one_instance_reaches_another() {
    let (a, _, b, b_state) = two_instances().await;
    send(&a, "PUT", "/v1/admin/providers/openai", &[], Some(json!({ "api_key": "sk-test-123456" }))).await;
    assert!(litegen::management::runtime::refresh(&b_state).await.unwrap());
    let (_, _, view) = send(&b, "GET", "/v1/admin/providers/openai", &[], None).await; assert_eq!(view["active"], true, "{view}");
}
```

The `PUT` body for a provider has to be the shape `put_provider` accepts, so
copy it from an existing provider test in `management/tests.rs`. Write the
Redis-gated sibling in `tests/scaling_replicas.rs` (Task 11): the change
arrives in under 1 s with `subscribe = true`.

- [ ] **Step 2: Run the tests and confirm they fail**

Run: `cargo test --test gateway_settings_api`
Expected: the three new tests fail because B never sees the change. The
existing tests pass.

- [ ] **Step 3: Implement** `events.rs`, the wider `refresh`, `AppliedRuntime`, `disabled_ids`, `spawn_runtime_sync`, `announce`, the nine handler calls and the `main.rs` line.
- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --test gateway_settings_api && cargo test --lib management:: && cargo test --lib engine_boundary`
Expected: PASS.

- [ ] **Step 5: Update the docs**, as listed under Files.
- [ ] **Step 6: Ledger entry**

---

### Task 8: The two-tier image cache

**Files:**
- Create: `litegen-core/src/shared/cache_tier.rs`
- Modify: `litegen-core/src/proxy/cache.rs`:
  - add the field `shared: std::sync::OnceLock<Arc<SharedRedis>>` and the field `seen_epoch: AtomicI64`;
  - add `attach_shared`, `clear_local` and `sync_epoch`;
  - extend `get_image`, `put_image`, `invalidate` and `clear`.
- Modify: `litegen-core/src/proxy/router.rs`: `with_shared` also calls `self.cache.attach_shared(shared.clone())`.
- Modify: `litegen-core/src/api/handlers/mod.rs:4339` (`clear_cache`): after `clear()`, call `announce(&state, SharedEvent::CacheClear).await`.
- Modify: `litegen-core/src/api/handlers/management/admin.rs`. When `put_settings` turns the cache off, the saving replica calls `state.router.cache.clear().await` before it announces `Reload`.
- Modify: `litegen-core/src/management/runtime.rs`:
  - `refresh` calls `state.router.cache.sync_epoch().await`;
  - the listener in `spawn_runtime_sync` handles `CacheClear` with `cache.sync_epoch()`.
- Modify: `docs/gateway-settings.md:66` and `:70`. With Redis, the cache has a shared tier behind the replica's own. `cache_entries` still counts this replica.

**Interfaces:**
- Consumes: `SharedRedis` (Task 2), `SharedEvent` and `announce` (Task 7).
- Produces:
  - `pub fn GenerationCache::attach_shared(&self, shared: Arc<SharedRedis>)`
  - `pub async fn clear(&self)`: clears the local tier and the shared tier, and bumps the epoch.
  - `pub async fn clear_local(&self)`
  - `pub async fn sync_epoch(&self)`: reads the epoch, and if it differs from `seen_epoch`, runs `clear_local` and records it. A missing key reads as 0.
  - `pub const MAX_SHARED_ENTRY_BYTES: usize = 262_144`
  - `shared::cache_tier::get(shared, sha: &str) -> Result<Option<(ImageGenerationResponse, Duration)>, SharedError>`, which returns the remaining TTL.
  - `shared::cache_tier::put(shared, sha, response, ttl) -> Result<(), SharedError>`. It stores JSON `{ "response": …, "expires_ms": … }` with `PX` set to `ttl`.
  - `shared::cache_tier::clear(shared) -> Result<(), SharedError>`. It loops `SCAN MATCH <prefix>cache:e:* COUNT 500` and `UNLINK`, then runs `INCR <prefix>cache:epoch`.
  - `shared::cache_tier::epoch(shared) -> Result<i64, SharedError>`
- **Lookups:** a local hit returns as today. On a local miss, the shared tier is read, and a hit is copied into the local tier with its remaining TTL. A store writes both tiers, but skips the shared one when the serialized answer is over `MAX_SHARED_ENTRY_BYTES`. Any `SharedError` counts as a miss or is skipped: the cache never blocks and never refuses. `reconfigure` stays local, as it is today.

- [ ] **Step 1: Write the failing tests** (in `cache_tier.rs`, Redis-gated)

```rust
#[tokio::test] async fn an_answer_cached_on_one_replica_is_a_hit_on_another() {
    let Some((sa, sb)) = crate::shared::testing::two_shared().await else { return };
    let (a, b) = (cache_with(sa), cache_with(sb));
    cache_put(&a, "prompt-1", small_response(), Duration::from_secs(60)).await;
    assert!(cache_get(&b, "prompt-1").await.is_some());
    assert_eq!(b.entry_count().await, 1, "copied into B's local tier");
}
#[tokio::test] async fn clear_on_one_replica_clears_the_other() {
    let Some((sa, sb)) = crate::shared::testing::two_shared().await else { return };
    let (a, b) = (cache_with(sa), cache_with(sb));
    cache_put(&a, "p", small_response(), Duration::from_secs(60)).await; assert!(cache_get(&b, "p").await.is_some());
    a.clear().await; b.sync_epoch().await;
    assert!(cache_get(&b, "p").await.is_none()); assert_eq!(b.entry_count().await, 0);
}
#[tokio::test] async fn oversized_answers_stay_local() {
    let Some((sa, sb)) = crate::shared::testing::two_shared().await else { return };
    let (a, b) = (cache_with(sa), cache_with(sb));
    cache_put(&a, "big", response_with_b64(MAX_SHARED_ENTRY_BYTES + 1), Duration::from_secs(60)).await;
    assert!(cache_get(&a, "big").await.is_some()); assert!(cache_get(&b, "big").await.is_none());
}
#[tokio::test] async fn a_dead_redis_is_a_miss_not_an_error() {
    let c = cache_with(crate::shared::testing::dead_redis(false).await);
    assert!(cache_get(&c, "nothing").await.is_none());
    cache_put(&c, "x", small_response(), Duration::from_secs(60)).await; assert!(cache_get(&c, "x").await.is_some(), "local still works");
}
#[tokio::test] async fn a_missing_epoch_reads_as_zero() {
    let Some((s, _)) = crate::shared::testing::two_shared().await else { return };
    assert_eq!(epoch(&s).await.unwrap(), 0);
}
```

`cache_with(shared)` is `GenerationCache::new(&enabled_config())` followed by
`attach_shared(shared)`. `cache_put` and `cache_get` are test helpers that wrap
`put_image` and `get_image` with a fixed request built from the prompt string.
They're named apart from the tier's own `put` and `get`.

- [ ] **Step 2: Run the tests and confirm they fail.** Run `cargo test --lib cache`. Expected: compile failure.
- [ ] **Step 3: Implement** as listed under Interfaces.
- [ ] **Step 4: Run the tests and confirm they pass**

Run: `cargo test --lib cache` with Redis up, then `cargo test --test gateway_settings_api`.
Expected: PASS. `the_cache_header_says_hit_miss_or_bypass` passes unchanged.

- [ ] **Step 5: Update the docs**, as listed under Files.
- [ ] **Step 6: Ledger entry**

---

### Task 9: Poller claims

**Files:**
- Create: `litegen-core/migrations/postgres/20240101000023_poll_claims.sql` and `litegen-core/migrations/sqlite/20240101000023_poll_claims.sql`. Each adds `poll_claimed_by TEXT NULL` and `poll_claimed_until`, typed `TIMESTAMPTZ NULL` in Postgres and `TIMESTAMP NULL` in SQLite. The SQLite file also adds `CREATE INDEX IF NOT EXISTS idx_generations_active ON generations(status, created_at)`. The Postgres file adds no index, because its claim query uses the existing `idx_generations_status` (ruling R35 FINAL in the ledger: building that index would block writes on a large `generations` table, and both concurrent-build shapes fail under sqlx 0.8). Copy the comment style of `…021_files_deleted_at.sql`.
- Modify: `litegen-core/src/db/trait_def.rs`: add `claim_active_generations`, with a default that calls `list_active_generations` for test doubles.
- Modify: `litegen-core/src/db/sqlite.rs` and `litegen-core/src/db/postgres.rs`: overrides next to `list_active_generations` (sqlite.rs:518, postgres.rs:522).
- Modify: `litegen-core/src/proxy/poller.rs`:
  - add `poll_once_as`;
  - `poll_once_presenting` becomes `poll_once_as(crate::shared::replica_id(), …)`;
  - add `POLL_CLAIM_HOLD`.
- Test: `poller.rs` `poller_tests` (SQLite) and `db/postgres_tests.rs` (gated).

**Interfaces:**
- Produces:
  - `async fn claim_active_generations(&self, limit: u32, claimant: &str, hold: chrono::Duration) -> Result<Vec<Generation>, sqlx::Error>`. It claims, oldest first, up to `limit` active rows (`pending` or `processing`) where `poll_claimed_until` is NULL, or before the database's `now()`, or `poll_claimed_by = claimant`. It sets `poll_claimed_by = claimant` and `poll_claimed_until = now() + hold`, and returns the rows.
    - Postgres: `UPDATE … WHERE id IN (SELECT id … ORDER BY created_at LIMIT $n FOR UPDATE SKIP LOCKED) RETURNING <GENERATION_COLS>`.
    - SQLite: the same, without `SKIP LOCKED`.
    - Use the database clock in both, never the app's.
  - `pub async fn poll_once_as(claimant: &str, db: &Arc<dyn DatabaseStore>, registry: &Arc<ProviderRegistry>, secrets_key: Option<[u8; 32]>, mode: Mode, model3d_store: &Arc<dyn ImageStorage>, presenting: Option<&Presenting>)`
  - `pub const POLL_CLAIM_HOLD: chrono::Duration = chrono::Duration::seconds(30)`
  - A replica keeps its rows from tick to tick, and a dead replica's rows pass to another replica 30 s after its last claim.

- [ ] **Step 1: Write the failing tests** (in `poller_tests`, SQLite)

```rust
#[tokio::test] async fn two_claimants_take_disjoint_rows() {
    let db = sqlite_with_active_rows(6).await;
    let a = db.claim_active_generations(3, "a", POLL_CLAIM_HOLD).await.unwrap();
    let b = db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap();
    assert_eq!((a.len(), b.len()), (3, 3)); assert!(a.iter().all(|r| b.iter().all(|s| s.id != r.id)));
}
#[tokio::test] async fn a_claimant_keeps_its_rows_tick_after_tick() {
    let db = sqlite_with_active_rows(2).await;
    let first = ids(db.claim_active_generations(10, "a", POLL_CLAIM_HOLD).await.unwrap());
    assert_eq!(ids(db.claim_active_generations(10, "a", POLL_CLAIM_HOLD).await.unwrap()), first);
    assert!(db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap().is_empty());
}
#[tokio::test] async fn a_lapsed_claim_passes_to_another_claimant() {
    let db = sqlite_with_active_rows(1).await;
    db.claim_active_generations(10, "a", chrono::Duration::milliseconds(200)).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await; // SQLite's CURRENT_TIMESTAMP has 1 s resolution
    assert_eq!(db.claim_active_generations(10, "b", POLL_CLAIM_HOLD).await.unwrap().len(), 1);
}
#[tokio::test] async fn terminal_rows_are_never_claimed() {
    let db = sqlite_with_rows(&["completed", "failed", "cancelled"]).await;
    assert!(db.claim_active_generations(10, "a", POLL_CLAIM_HOLD).await.unwrap().is_empty());
}
#[tokio::test] async fn two_pollers_poll_each_job_once_per_tick() {
    // Counting mock 3D provider (like GatedModel3dProvider, line 2423) over four active rows.
    poll_once_as("a", &db, &registry, None, Mode::SingleTenant, &store, None).await;
    poll_once_as("b", &db, &registry, None, Mode::SingleTenant, &store, None).await;
    assert!(provider.polls_per_job().values().all(|n| *n == 1));
}
```

Add `concurrent_claims_never_overlap` to `db/postgres_tests.rs`, gated on
`LITEGEN_PG_TEST_URL`: run 20 rows and two concurrent
`claim_active_generations(10, …)` calls with `tokio::join!`, and assert the
two results are disjoint and add up to 20.

- [ ] **Step 2: Run the tests and confirm they fail.** Run `cargo test --lib poller_tests`. Expected: compile failure.
- [ ] **Step 3: Implement** the migrations, the trait method and both overrides, `poll_once_as` and the constant.
- [ ] **Step 4: Run the tests and confirm they pass**

Run `cargo test --lib poller_tests`. Then start the Postgres container
(`.claude/rules/database-tests.md`) and run
`LITEGEN_PG_TEST_URL=… cargo test --lib`.
Expected: PASS.

- [ ] **Step 5: Ledger entry**

---

### Task 10: The stored link-signing key

**Files:**
- Create: `litegen-core/migrations/postgres/20240101000024_server_secrets.sql` and the same file under `sqlite/`: `CREATE TABLE IF NOT EXISTS server_secrets (name TEXT PRIMARY KEY, value TEXT NOT NULL, created_at <TIMESTAMPTZ|TIMESTAMP> NOT NULL DEFAULT CURRENT_TIMESTAMP)`.
- Modify: `litegen-core/src/db/trait_def.rs`: add `server_secret`. The default calls `generate()` every time, which suits test doubles. Override it in `sqlite.rs` and `postgres.rs` with `INSERT … ON CONFLICT (name) DO NOTHING`, then a `SELECT`.
- Modify: `litegen-core/src/access/signing.rs`: add `from_stored_key` and `link_signer_for`.
- Modify: `litegen-core/src/proxy/router.rs`: add `pub fn with_link_signer(mut self, signer: Arc<LinkSigner>) -> Self`.
- Modify: `litegen-core/src/main.rs`: `ProxyRouter::new(..).with_link_signer(Arc::new(link_signer_for(config.secrets_key.as_deref(), &*db).await))`.

**Interfaces:**
- Produces:
  - `async fn server_secret(&self, name: &str, generate: &(dyn Fn() -> String + Send + Sync)) -> Result<String, sqlx::Error>`
  - `pub fn LinkSigner::from_stored_key(stored_b64: &str) -> Option<LinkSigner>`: 32 bytes of base64, with `persistent: true`.
  - `pub async fn link_signer_for(secrets_key: Option<&str>, db: &dyn DatabaseStore) -> LinkSigner`. A secrets key that decodes wins and goes through `from_secrets_key`. Otherwise the signer uses `server_secret("link_signing_key", random 32 bytes as base64)`. If the database fails, it logs and falls back to `from_secrets_key(None)`, which is today's random key.

- [ ] **Step 1: Write the failing tests** (in `signing.rs`, SQLite file)

```rust
#[tokio::test] async fn two_signers_over_one_database_agree() {
    let (db1, db2) = two_connections_to_one_sqlite_file().await;
    let (a, b) = (link_signer_for(None, &*db1).await, link_signer_for(None, &*db2).await);
    assert!(a.persistent() && b.persistent()); assert!(b.verify("k", 99, &a.sign("k", 99)));
}
#[tokio::test] async fn the_secrets_key_still_wins() {
    let (db, _) = two_connections_to_one_sqlite_file().await; let key = base64_32(7);
    assert_eq!(link_signer_for(Some(&key), &*db).await.sign("k", 1), LinkSigner::from_secrets_key(Some(&key)).sign("k", 1));
}
#[tokio::test] async fn a_restart_keeps_the_key() {
    let path = temp_sqlite_path();
    let s1 = link_signer_for(None, &*open(&path).await).await.sign("k", 5);
    assert_eq!(link_signer_for(None, &*open(&path).await).await.sign("k", 5), s1);
}
```

Add `server_secret_is_created_once` to `db/postgres_tests.rs`: two
concurrent calls with different generators both return the same value.

- [ ] **Step 2: Run the tests and confirm they fail.** Run `cargo test --lib signing`. Expected: compile failure.
- [ ] **Step 3: Implement** as listed under Files and Interfaces.
- [ ] **Step 4: Run the tests and confirm they pass**

Run `cargo test --lib signing storage`, then the gated Postgres tests:
`LITEGEN_PG_TEST_URL=… cargo test --lib`.
Expected: PASS.

- [ ] **Step 5: Ledger entry**

---

### Task 11: The two-replica proof

**Files:**
- Modify: `litegen-core/tests/harness/mod.rs`:
  - `build_app_in` takes `db: Arc<dyn DatabaseStore>`, and callers coerce;
  - add `pub struct ReplicaOptions { pub subscribe: bool }`, whose `Default` has `subscribe: true`;
  - add `pub async fn replica(db: Arc<dyn DatabaseStore>, shared: Option<Arc<SharedRedis>>, asset_dir: &Path, opts: ReplicaOptions, tweak: impl FnOnce(&mut AppConfig)) -> (Router, Arc<AppState>)`. It wires like `main.rs`:
    - `ProxyRouter::new(..).with_shared(s).with_link_signer(Arc::new(link_signer_for(None, &*db).await))`;
    - `RateLimiter::with_shared(s)`;
    - `spawn_runtime_sync(state, Duration::from_secs(15), opts.subscribe)`.
- Create: `litegen-core/tests/scaling_replicas.rs`. It's gated on both `LITEGEN_PG_TEST_URL` and `LITEGEN_REDIS_TEST_URL`, and skips with a message when either is missing.

**Interfaces:**
- Consumes everything from Tasks 1 to 10.
- Each test creates its own database on the test server. It connects to `LITEGEN_PG_TEST_URL`, runs `CREATE DATABASE litegen_scaling_<uuid>`, and connects both replicas to it, so migrations run fresh. It drops the database at the end. Each test also uses its own Redis prefix, from `shared::testing::test_prefix()` through `SharedRedis::connect`.

- [ ] **Step 1: Write the tests**, one per scenario in the spec's Proof section:
  - `a_keys_rpm_is_shared_by_both_replicas`: a key with `rpm_limit` 4. Requests alternate between A and B: 4 succeed, and the fifth gets 429 with `retry-after`.
  - `a_parallel_cap_holds_across_replicas`: `max_parallel_requests` 1, using the `mock/slow-image` model. One request is in flight on A, so B gets 429 `parallel_limit_exceeded`. Once A's request finishes, B succeeds. A crashed replica's slots freeing themselves is proven at the Redis level by Task 4's `a_lost_holder_frees_its_slot_after_the_lease`.
  - `turning_the_cache_off_on_a_empties_the_shared_tier`: cache an answer through A, `PUT /v1/admin/settings {"cache_enabled": false}` on A, then turn it back on. B answers `miss` for the same request.
  - `a_model_disabled_on_a_is_refused_on_b_within_a_second`. Then the same with `ReplicaOptions { subscribe: false }` and `refresh` called by hand.
  - `each_active_job_is_polled_by_one_replica_per_tick`: two `poll_once_as` calls with different claimants against a counting mock.
  - `a_link_signed_on_a_opens_on_b`: generate on A with `keep_outputs`, then `GET` the signed asset URL through B and get 200.
  - `with_redis_cut_requests_succeed_on_local_limits_and_fail_closed_refuses`: route Redis through `CutProxy`, `cut()`, and expect 200 under the local limits. A replica built with `fail_closed: true` gets 503 `rate_limit_unverifiable`. After `restore()` and the breaker's recovery (set to 1 s in the test), the limits are shared again.
  - `clearing_the_cache_on_a_clears_b_and_an_answer_cached_on_a_hits_on_b`: check `X-Litegen-Cache` on B, `miss` then `hit`. After `DELETE /v1/cache` on A, B answers `miss` within 1 s.
  - `a_provider_that_keeps_failing_stays_cut_off_on_both_replicas`: a mock provider that always fails, threshold 2, `open_for` 1 s. After the window, one failed probe on A opens the breaker again for B.

- [ ] **Step 2: Run the tests**

Run, with both containers up:
`LITEGEN_PG_TEST_URL=… LITEGEN_REDIS_TEST_URL=… cargo test --test scaling_replicas`.
Expected: PASS. Without the variables, every test prints its skip line and
passes.

- [ ] **Step 3: Run the harness's other users**

Run: `cargo test --test gateway_settings_api --test hooks_api --test model3d_api`
Expected: PASS. This proves the harness change kept its callers working.

- [ ] **Step 4: Ledger entry**

---

### Task 12: CI, docs and the full gate

**Files:**
- Modify: `.github/workflows/ci.yml`:
  - under `core.services`, add `redis: { image: redis:7-alpine, ports: ['6379:6379'], options: --health-cmd "redis-cli ping" --health-interval 5s --health-timeout 5s --health-retries 30 }`;
  - under `core.env`, add `LITEGEN_REDIS_TEST_URL: redis://127.0.0.1:6379`.
- Modify: `docs/ci.md`: the Redis service and its variable.
- Modify: `README.md:683-686`. Replace the sticky-sessions paragraph: set Postgres, S3 and a Redis URL, and link `docs/scaling.md`. Also update `README.md:832-834`: the roadmap item is done.
- Modify: `docs/RUNBOOK.md:67`: rate limits are shared through Redis when it's set.
- Modify: `docs/scaling.md`. Add these sections:
  - "Several replicas": what's shared, and how;
  - "When Redis is down": the fallback, fail-closed and the breaker, as in the spec's Redis outage section;
  - "Running Redis": a dedicated Redis or database index, and `maxmemory-policy volatile-lru`, because every litegen key except the cache epoch has a TTL;
  - "Startup messages".

- [ ] **Step 1: Make the CI and docs edits** listed under Files.
- [ ] **Step 2: Lint the workflow**, as `docs/ci.md` says (the `actionlint` job's command).
- [ ] **Step 3: Run the full gate**, as `docs/ci.md` describes and CI runs it:
  - clippy: `cargo clippy --locked --all-targets --all-features -- -D warnings`;
  - the full test run with both test containers up and every `LITEGEN_*_TEST*` variable set;
  - the three timing tests on their own, skipped in the main run, as the CI step does (see `TIMING_TESTS` in `ci.yml`).

  Expected: zero warnings, and every test passes.
- [ ] **Step 4: Check that nothing reaches the API**

Run: `git diff --stat -- ../apps/landing/public/openapi.json ../sdks`
Expected: nothing changed. Also check that the MCP `tools_reference_is_current`
test passed in step 3.

- [ ] **Step 5: Final ledger entry.** Record the gate results and the files changed (`git status --short`, filtered to this plan's paths), and say that nothing is committed.
