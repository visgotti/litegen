//! Hook processes in every runtime: the runner's protocol, refusals and
//! errors, overlapping calls, timeouts, crashes and restarts, the
//! environment, and that a hook never outlives litegen. Each test runs over
//! `RUNTIMES`; a runtime whose program isn't installed is skipped loudly.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use litegen::hooks::process::{write_runner, HookCallError, HookFn, HookOutcome, HookProcess, Refusal};
use litegen::hooks::{HookConfig, ResolvedHook, Runtime};
use serde_json::{json, Value};

struct Rt {
    label: &'static str,
    behaviour: &'static str,
    /// Extension of the load-failure fixtures (`neither`, `broken`, `pre_only`).
    load_ext: &'static str,
    runtime: Runtime,
    /// Set when the runtime differs from what the extension implies.
    named: Option<&'static str>,
    /// How this runtime names a file that won't parse.
    parse_error: &'static str,
}

const RUNTIMES: &[Rt] = &[
    Rt { label: "python", behaviour: "behaviour.py", load_ext: "py", runtime: Runtime::Python, named: None, parse_error: "SyntaxError" },
    Rt { label: "node", behaviour: "behaviour.mjs", load_ext: "mjs", runtime: Runtime::Node, named: None, parse_error: "SyntaxError" },
    Rt { label: "bun", behaviour: "behaviour.mjs", load_ext: "mjs", runtime: Runtime::Bun, named: Some("bun"), parse_error: "BuildMessage" },
    Rt { label: "bun-ts", behaviour: "behaviour.ts", load_ext: "mjs", runtime: Runtime::Bun, named: Some("bun"), parse_error: "BuildMessage" },
    // Node runs TypeScript itself from 22.18 (type stripping); see installed().
    Rt { label: "node-ts", behaviour: "behaviour.ts", load_ext: "mjs", runtime: Runtime::Node, named: Some("node"), parse_error: "SyntaxError" },
];

const T: Duration = Duration::from_secs(2);

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hooks")
}

fn installed(rt: &Rt) -> bool {
    let version = std::process::Command::new(rt.runtime.default_program())
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_start_matches('v').to_string());
    let Some(version) = version else {
        eprintln!("SKIPPED: {} not installed", rt.label);
        return false;
    };
    if rt.label == "node-ts" {
        let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        if (major, minor) < (22, 18) {
            eprintln!("SKIPPED: node-ts needs Node 22.18+ to strip types; found {version}");
            return false;
        }
    }
    true
}

fn resolved(rt: &Rt, file: &str, extra: Value) -> ResolvedHook {
    let mut v = json!({
        "path": file,
        "env": { "HOOK_LABEL": rt.label, "HOOK_OWN_VAR": "own-value", "TEST_LITEGEN_KEY": "test-master" },
    });
    if let Some(r) = rt.named {
        v["runtime"] = json!(r);
    }
    for (k, val) in extra.as_object().cloned().unwrap_or_default() {
        v[k] = val;
    }
    serde_json::from_value::<HookConfig>(v).unwrap().resolve(&fixtures()).unwrap()
}

async fn behaviour(rt: &Rt) -> Arc<HookProcess> {
    HookProcess::start(&resolved(rt, rt.behaviour, json!({}))).await.unwrap_or_else(|e| panic!("{}: {e}", rt.label))
}

fn request(action: &str, post: &str, extra: &[(&str, &str)]) -> Value {
    let mut headers = json!({ "x-test-action": action, "x-test-post": post });
    for (k, v) in extra {
        headers[*k] = json!(v);
    }
    json!({
        "method": "POST", "path": "/v1/images/generations", "query": "",
        "headers": headers, "body": { "prompt": "a fox" }, "files": [],
    })
}

fn response() -> Value {
    json!({ "status": 200, "headers": { "content-type": "application/json" }, "body": { "data": [] } })
}

fn caller() -> Value {
    json!({ "key_id": null, "org_id": null, "app_id": null, "end_user": null, "user": null })
}

async fn pre(p: &HookProcess, req: Value, timeout: Duration) -> Result<HookOutcome, HookCallError> {
    p.call(HookFn::PreCall, vec![req], timeout).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_runner_reports_which_hooks_a_file_defines() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let only = HookProcess::start(&resolved(rt, &format!("pre_only.{}", rt.load_ext), json!({}))).await.unwrap();
        assert!(only.defines(HookFn::PreCall), "{}", rt.label);
        assert!(!only.defines(HookFn::PostCall) && !only.defines(HookFn::FileAccess), "{}", rt.label);
        let all = behaviour(rt).await;
        assert!(all.defines(HookFn::PreCall) && all.defines(HookFn::PostCall) && all.defines(HookFn::FileAccess), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_access_only_hook_starts_in_every_runtime() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let only = HookProcess::start(&resolved(rt, &format!("file_only.{}", rt.load_ext), json!({}))).await.unwrap();
        assert!(only.defines(HookFn::FileAccess), "{}", rt.label);
        assert!(!only.defines(HookFn::PreCall) && !only.defines(HookFn::PostCall), "{}", rt.label);
        let file = json!({ "generation_id": "g", "index": 0 });
        let ask = |headers: Value| only.call(HookFn::FileAccess, vec![json!({ "headers": headers }), file.clone(), json!({})], T);
        // `false` refuses with 403; nothing serves.
        match ask(json!({ "x-test-file": "deny" })).await {
            Ok(HookOutcome::Refused(r)) => assert_eq!(r.status, 403, "{}: {r:?}", rt.label),
            other => panic!("{}: {other:?}", rt.label),
        }
        assert_eq!(ask(json!({})).await.unwrap(), HookOutcome::Unchanged, "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_access_answers_yes_no_or_a_refusal() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let file = json!({ "generation_id": "g", "index": 0, "created_by": { "end_user": "alice" } });
        let ask = |action: &str| {
            let req = json!({ "headers": { "x-test-file": action, "x-litegen-end-user": "bob" } });
            p.call(HookFn::FileAccess, vec![req, file.clone(), json!({})], T)
        };
        let refusal = |o: Result<HookOutcome, HookCallError>| match o {
            Ok(HookOutcome::Refused(r)) => (r.status, r.message),
            other => panic!("{}: {other:?}", rt.label),
        };
        assert_eq!(refusal(ask("deny-false").await).0, 403, "{}: false", rt.label);
        // A string refuses a file with 403, not the 400 a generation hook's does.
        assert_eq!(refusal(ask("deny-string").await), (403, "files are closed today".to_string()), "{}", rt.label);
        assert_eq!(refusal(ask("deny-other-end-user").await), (403, "not your file".to_string()), "{}", rt.label);
        assert_eq!(ask("none").await.unwrap(), HookOutcome::Unchanged, "{}", rt.label);
        assert!(matches!(ask("fail").await, Err(HookCallError::Raised(_))), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_with_neither_hook_fails_to_start() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let err = HookProcess::start(&resolved(rt, &format!("neither.{}", rt.load_ext), json!({}))).await.err().unwrap();
        assert!(err.contains("defines none of pre_call, post_call and file_access"), "{}: {err}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_that_fails_to_load_fails_to_start() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let err = HookProcess::start(&resolved(rt, &format!("broken.{}", rt.load_ext), json!({}))).await.err().unwrap();
        assert!(err.contains(rt.parse_error), "{}: {err}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_and_async_hooks_both_answer() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        // pre_call is async and post_call sync in every behaviour fixture.
        assert_eq!(pre(&p, request("", "", &[]), T).await.unwrap(), HookOutcome::Unchanged, "{}", rt.label);
        let post = p.call(HookFn::PostCall, vec![request("", "", &[]), response(), caller()], T).await.unwrap();
        assert_eq!(post, HookOutcome::Unchanged, "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_raised_status_is_a_refusal_with_its_status_message_and_headers() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let got = pre(&p, request("refuse-status", "", &[]), T).await.unwrap();
        let want = Refusal { status: 401, message: "who are you".into(), headers: vec![("www-authenticate".into(), "Test".into())] };
        assert_eq!(got, HookOutcome::Refused(want), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_returned_string_is_a_400_refusal() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let got = pre(&p, request("refuse-string", "", &[]), T).await.unwrap();
        assert_eq!(got, HookOutcome::Refused(Refusal { status: 400, message: "not today".into(), headers: vec![] }), "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn any_other_exception_is_a_hook_error_with_its_message() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let got = p.call(HookFn::PostCall, vec![request("", "fail", &[]), response(), caller()], T).await;
        match got {
            Err(HookCallError::Raised(m)) => assert!(m.contains("boom"), "{}: {m}", rt.label),
            other => panic!("{}: expected Raised, got {other:?}", rt.label),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hook_that_prints_still_answers() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        assert_eq!(pre(&p, request("print", "", &[]), T).await.unwrap(), HookOutcome::Unchanged, "{}", rt.label);
        let after = pre(&p, request("echo-id", "", &[("x-id", "after-print")]), T).await.unwrap();
        match after {
            HookOutcome::Replaced(v) => assert_eq!(v["headers"]["x-answer"], "after-print", "{}", rt.label),
            other => panic!("{}: expected Replaced, got {other:?}", rt.label),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_calls_are_matched_to_their_own_answers() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let calls = (0..20).map(|i| {
            let p = p.clone();
            async move {
                let id = i.to_string();
                // Even calls sleep, so their answers come back after the odd ones'.
                let action = if i % 2 == 0 { "sleep-300,echo-id" } else { "echo-id" };
                (id.clone(), pre(&p, request(action, "", &[("x-id", &id)]), T).await)
            }
        });
        for (id, got) in futures::future::join_all(calls).await {
            match got {
                Ok(HookOutcome::Replaced(v)) => assert_eq!(v["headers"]["x-answer"], json!(id), "{}", rt.label),
                other => panic!("{}: call {id}: {other:?}", rt.label),
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_past_its_timeout_is_a_timeout_and_the_next_call_still_works() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let slow = pre(&p, request("sleep", "", &[]), Duration::from_millis(200)).await;
        assert!(matches!(slow, Err(HookCallError::Timeout)), "{}: {slow:?}", rt.label);
        assert_eq!(pre(&p, request("", "", &[]), T).await.unwrap(), HookOutcome::Unchanged, "{}", rt.label);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crashed_hook_is_restarted_and_answers_again() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let crashed = pre(&p, request("crash", "", &[]), T).await;
        assert!(matches!(crashed, Err(HookCallError::Crashed(_))), "{}: {crashed:?}", rt.label);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match pre(&p, request("", "", &[]), T).await {
                Ok(HookOutcome::Unchanged) => break,
                other if Instant::now() > deadline => panic!("{}: not restarted within 5 s: {other:?}", rt.label),
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hook_inherits_litegens_environment_plus_its_own_env() {
    let inherited = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR for test runs");
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        match pre(&p, request("env", "", &[]), T).await.unwrap() {
            HookOutcome::Refused(r) => {
                assert!(r.message.contains(&format!("inherited={inherited}")), "{}: {}", rt.label, r.message);
                assert!(r.message.contains("own=own-value"), "{}: {}", rt.label, r.message);
            }
            other => panic!("{}: expected Refused, got {other:?}", rt.label),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hook_never_outlives_its_owner() {
    let alive = |pid: u32| std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false);
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        // Dropping the last handle kills the child.
        let p = behaviour(rt).await;
        let pid = p.pid().expect("a running hook has a pid");
        assert!(alive(pid), "{}", rt.label);
        drop(p);
        let deadline = Instant::now() + Duration::from_secs(2);
        while alive(pid) {
            assert!(Instant::now() < deadline, "{}: hook {pid} outlived its HookProcess", rt.label);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // A runner whose stdin closes exits on its own, as after litegen is SIGKILLed.
        let dir = tempfile::tempdir().unwrap();
        let runner = write_runner(dir.path(), rt.runtime).unwrap();
        let mut child = std::process::Command::new(rt.runtime.default_program())
            .arg(&runner)
            .arg(fixtures().join(rt.behaviour))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut ready = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready).unwrap();
        assert!(ready.contains("\"ready\":true"), "{}: {ready}", rt.label);
        drop(child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "{}: the runner kept running after its stdin closed", rt.label);
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A hook's directory is the operator's: the Python runner must not leave
/// bytecode caches (`__pycache__`) beside the hook file, which would litter a
/// repo or fail in a read-only one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_python_runner_leaves_no_bytecode_beside_the_hook() {
    let rt = &RUNTIMES[0];
    if !installed(rt) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(fixtures().join("behaviour.py"), dir.path().join("behaviour.py")).unwrap();
    let hook = serde_json::from_value::<HookConfig>(json!({ "path": "behaviour.py" })).unwrap().resolve(dir.path()).unwrap();
    let p = HookProcess::start(&hook).await.unwrap();
    assert_eq!(pre(&p, request("", "", &[]), T).await.unwrap(), HookOutcome::Unchanged);
    drop(p);
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(left, ["behaviour.py"], "the hook's directory holds only the hook");
}

/// A hook stuck in a loop never exits, so the crash restart can't see it:
/// after three timeouts in a row litegen kills it and starts it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hung_hook_is_restarted_after_three_timeouts_in_a_row() {
    for rt in RUNTIMES.iter().filter(|rt| installed(rt)) {
        let p = behaviour(rt).await;
        let hung_pid = p.pid().unwrap();
        let short = Duration::from_millis(200);
        assert!(matches!(pre(&p, request("hang", "", &[]), short).await, Err(HookCallError::Timeout)), "{}", rt.label);
        for _ in 0..2 {
            // The loop is blocked: these time out too.
            assert!(matches!(pre(&p, request("", "", &[]), short).await, Err(HookCallError::Timeout)), "{}", rt.label);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match pre(&p, request("", "", &[]), T).await {
                Ok(HookOutcome::Unchanged) => break,
                other if Instant::now() > deadline => panic!("{}: not restarted within 5 s: {other:?}", rt.label),
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        assert_ne!(p.pid(), Some(hung_pid), "{}: a fresh process answers", rt.label);
    }
}
