//! One hook file's long-lived child process: started once, called many times
//! with overlapping calls, restarted when it dies, and killed with its owner.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::config::{ResolvedHook, Runtime};
use super::protocol::{Answer, Call, Ready};

const RUNNER_PY: &str = include_str!("runners/runner.py");
const RUNNER_MJS: &str = include_str!("runners/runner.mjs");

/// How long a starting runner may take to load the hook file.
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Restart backoff: from the first value, doubling, up to the second.
const RESTART_BACKOFF: (Duration, Duration) = (Duration::from_millis(100), Duration::from_secs(5));
/// Lines of a hook's stderr kept for error messages.
const STDERR_TAIL: usize = 5;
/// Timeouts in a row after which a hook counts as hung: litegen kills it,
/// and the supervisor starts it again.
const HANG_LIMIT: u32 = 3;

/// Which hook function a call runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFn {
    PreCall,
    PostCall,
    /// Whether a stored file may be served (`docs/storage-access.md`).
    FileAccess,
}

impl HookFn {
    pub fn name(self) -> &'static str {
        match self {
            HookFn::PreCall => "pre_call",
            HookFn::PostCall => "post_call",
            HookFn::FileAccess => "file_access",
        }
    }
}

/// What a hook decided.
#[derive(Debug, Clone, PartialEq)]
pub enum HookOutcome {
    /// It returned nothing: carry on as before.
    Unchanged,
    /// It returned its argument, changed (still to be checked by the caller).
    Replaced(Value),
    /// It refused: raised an error carrying a status, or returned a string.
    Refused(Refusal),
}

/// A hook's refusal: what the caller gets back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    pub message: String,
    pub headers: Vec<(String, String)>,
}

/// A hook that failed to decide: its failure policy applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookCallError {
    Timeout,
    Crashed(String),
    Raised(String),
    Malformed(String),
}

impl std::fmt::Display for HookCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookCallError::Timeout => write!(f, "did not answer in time"),
            HookCallError::Crashed(m) => write!(f, "crashed: {m}"),
            HookCallError::Raised(m) => write!(f, "raised an error: {m}"),
            HookCallError::Malformed(m) => write!(f, "answered badly: {m}"),
        }
    }
}

impl std::error::Error for HookCallError {}

/// Write the runner for `runtime` into `dir`, returning its path.
pub fn write_runner(dir: &Path, runtime: Runtime) -> std::io::Result<PathBuf> {
    let (file, source) = match runtime {
        Runtime::Python => ("runner.py", RUNNER_PY),
        Runtime::Node | Runtime::Bun => ("runner.mjs", RUNNER_MJS),
    };
    let path = dir.join(file);
    std::fs::write(&path, source)?;
    Ok(path)
}

type Pending = HashMap<u64, (u64, oneshot::Sender<Result<Answer, HookCallError>>)>;

/// The live connection: the current child's stdin, and the calls waiting on
/// an answer, each tagged with the generation (child) it was sent to.
struct Conn {
    generation: u64,
    writer: Option<mpsc::UnboundedSender<String>>,
    pending: Pending,
}

struct Shared {
    name: String,
    conn: Mutex<Conn>,
    next_id: AtomicU64,
    pid: AtomicU32,
    stderr_tail: Mutex<VecDeque<String>>,
    /// Calls timed out since the hook last answered.
    timeouts_in_a_row: AtomicU32,
    /// Asks the supervisor to kill the running child (a hung hook).
    kill: tokio::sync::Notify,
}

impl Shared {
    fn tail(&self) -> String {
        let tail = self.stderr_tail.lock().unwrap();
        if tail.is_empty() { String::new() } else { format!("; last output: {}", tail.iter().cloned().collect::<Vec<_>>().join(" | ")) }
    }
}

/// A running hook file. Dropping the last handle kills the process.
pub struct HookProcess {
    shared: Arc<Shared>,
    defined: Vec<HookFn>,
    supervisor: tokio::task::JoinHandle<()>,
    runner_dir: PathBuf,
}

impl HookProcess {
    /// Start `hook`'s runner and wait for it to load the file.
    pub async fn start(hook: &ResolvedHook) -> Result<Arc<HookProcess>, String> {
        let runner_dir = private_dir().map_err(|e| format!("{}: cannot write its runner: {e}", hook.name))?;
        let runner = match write_runner(&runner_dir, hook.runtime) {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&runner_dir);
                return Err(format!("{}: cannot write its runner: {e}", hook.name));
            }
        };
        let shared = Arc::new(Shared {
            name: hook.name.clone(),
            conn: Mutex::new(Conn { generation: 0, writer: None, pending: HashMap::new() }),
            next_id: AtomicU64::new(1),
            pid: AtomicU32::new(0),
            stderr_tail: Mutex::new(VecDeque::new()),
            timeouts_in_a_row: AtomicU32::new(0),
            kill: tokio::sync::Notify::new(),
        });
        let (child, hooks) = match spawn(hook, &runner, &shared).await {
            Ok(started) => started,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&runner_dir);
                return Err(e);
            }
        };
        let defined = [HookFn::PreCall, HookFn::PostCall, HookFn::FileAccess].into_iter().filter(|f| hooks.iter().any(|h| h == f.name())).collect();
        let supervisor = tokio::spawn(supervise(shared.clone(), hook.clone(), runner, child));
        Ok(Arc::new(HookProcess { shared, defined, supervisor, runner_dir }))
    }

    /// The hook file's name.
    pub fn name(&self) -> &str {
        &self.shared.name
    }

    pub fn defines(&self, f: HookFn) -> bool {
        self.defined.contains(&f)
    }

    /// The current child's pid, if one is running.
    pub fn pid(&self) -> Option<u32> {
        Some(self.shared.pid.load(Ordering::SeqCst)).filter(|p| *p != 0)
    }

    /// Run `f` with `args`. Calls overlap; each waits at most `timeout`.
    pub async fn call(&self, f: HookFn, args: Vec<Value>, timeout: Duration) -> Result<HookOutcome, HookCallError> {
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        let line = serde_json::to_string(&Call { id, function: f.name(), args: &args })
            .map_err(|e| HookCallError::Malformed(format!("cannot encode the call: {e}")))?;
        let rx = {
            let mut conn = self.shared.conn.lock().unwrap();
            let Some(writer) = conn.writer.clone() else {
                return Err(HookCallError::Crashed("the hook process is restarting".into()));
            };
            let (tx, rx) = oneshot::channel();
            let generation = conn.generation;
            conn.pending.insert(id, (generation, tx));
            if writer.send(line + "\n").is_err() {
                conn.pending.remove(&id);
                return Err(HookCallError::Crashed("the hook process is restarting".into()));
            }
            rx
        };
        let answer = match tokio::time::timeout(timeout, rx).await {
            Err(_) => {
                self.shared.conn.lock().unwrap().pending.remove(&id);
                // A hook stuck in a loop never exits, so the crash restart
                // can't see it: enough timeouts in a row restart it instead.
                if self.shared.timeouts_in_a_row.fetch_add(1, Ordering::SeqCst) + 1 >= HANG_LIMIT {
                    self.shared.timeouts_in_a_row.store(0, Ordering::SeqCst);
                    tracing::warn!(target: "litegen::hooks", hook = %self.shared.name, "hook did not answer {HANG_LIMIT} calls in a row; restarting it");
                    self.shared.kill.notify_waiters();
                }
                return Err(HookCallError::Timeout);
            }
            Ok(Err(_)) => return Err(HookCallError::Crashed("the hook process exited".into())),
            Ok(Ok(answer)) => answer?,
        };
        self.shared.timeouts_in_a_row.store(0, Ordering::SeqCst);
        outcome(f, answer)
    }
}

impl Drop for HookProcess {
    fn drop(&mut self) {
        // The supervisor owns the child, spawned with kill_on_drop.
        self.supervisor.abort();
        {
            let mut conn = self.shared.conn.lock().unwrap();
            conn.writer = None;
            for (_, (_, tx)) in conn.pending.drain() {
                let _ = tx.send(Err(HookCallError::Crashed("the hook was shut down".into())));
            }
        }
        let _ = std::fs::remove_dir_all(&self.runner_dir);
    }
}

fn outcome(f: HookFn, answer: Answer) -> Result<HookOutcome, HookCallError> {
    if let Some(error) = answer.error {
        return Err(HookCallError::Raised(error));
    }
    if let Some(r) = answer.refuse {
        if !(400..=599).contains(&r.status) {
            return Err(HookCallError::Malformed(format!("refused with status {}; a refusal must be 400-599", r.status)));
        }
        return Ok(HookOutcome::Refused(Refusal { status: r.status as u16, message: r.message, headers: r.headers.into_iter().collect() }));
    }
    match (f, answer.result) {
        (_, None | Some(Value::Null)) => Ok(HookOutcome::Unchanged),
        // `file_access` answers yes or no.
        (HookFn::FileAccess, Some(Value::Bool(true))) => Ok(HookOutcome::Unchanged),
        (HookFn::FileAccess, Some(Value::Bool(false))) => Ok(HookOutcome::Refused(Refusal {
            status: 403,
            message: "the file_access hook refused this file".into(),
            headers: Vec::new(),
        })),
        (HookFn::FileAccess, Some(other)) => Err(HookCallError::Malformed(format!(
            "returned {other}; file_access returns nothing or true to serve the file, false or a string to refuse"
        ))),
        (_, Some(v @ Value::Object(_))) => Ok(HookOutcome::Replaced(v)),
        (_, Some(other)) => Err(HookCallError::Malformed(format!(
            "returned {other}; a hook returns its argument changed, a string to refuse, or nothing"
        ))),
    }
}

/// A fresh directory only this user can read, for one hook's runner.
fn private_dir() -> std::io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("litegen-hook-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    Ok(dir)
}

/// Start one child and wait for its `ready` line. On success the connection
/// points at the new child, under a new generation.
async fn spawn(hook: &ResolvedHook, runner: &Path, shared: &Arc<Shared>) -> Result<(Child, Vec<String>), String> {
    let mut child = Command::new(&hook.program)
        .arg(runner)
        .arg(&hook.path)
        .envs(&hook.env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("{}: cannot start {}: {e}", hook.name, hook.program))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    shared.stderr_tail.lock().unwrap().clear();
    tokio::spawn(forward_stderr(shared.clone(), stderr));

    let mut lines = BufReader::new(stdout).lines();
    let first = match tokio::time::timeout(READY_TIMEOUT, lines.next_line()).await {
        Err(_) => return Err(format!("{}: did not load within {} s", hook.name, READY_TIMEOUT.as_secs())),
        Ok(Err(e)) => return Err(format!("{}: unreadable startup: {e}", hook.name)),
        Ok(Ok(None)) => {
            let status = child.wait().await.map(|s| s.to_string()).unwrap_or_default();
            // Let the stderr forwarder catch up before quoting it.
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Err(format!("{}: exited while loading ({status}){}", hook.name, shared.tail()));
        }
        Ok(Ok(Some(line))) => line,
    };
    let ready: Ready = serde_json::from_str(&first).map_err(|e| format!("{}: sent {first:?} instead of ready: {e}", hook.name))?;
    if !ready.ready {
        return Err(format!("{}: {}", hook.name, ready.error.unwrap_or_else(|| "failed to load".into())));
    }

    let (tx, rx) = mpsc::unbounded_channel::<String>();
    tokio::spawn(write_lines(stdin, rx));
    let generation = {
        let mut conn = shared.conn.lock().unwrap();
        conn.generation += 1;
        conn.writer = Some(tx);
        conn.generation
    };
    shared.pid.store(child.id().unwrap_or(0), Ordering::SeqCst);
    shared.timeouts_in_a_row.store(0, Ordering::SeqCst);
    tokio::spawn(read_answers(shared.clone(), lines, generation));
    Ok((child, ready.hooks))
}

/// Restart the child whenever it exits, or when a call finds it hung, with
/// backoff, until aborted.
async fn supervise(shared: Arc<Shared>, hook: ResolvedHook, runner: PathBuf, mut child: Child) {
    let mut backoff = RESTART_BACKOFF.0;
    loop {
        let status = tokio::select! {
            status = child.wait() => status,
            _ = shared.kill.notified() => {
                let _ = child.start_kill();
                child.wait().await
            }
        };
        shared.pid.store(0, Ordering::SeqCst);
        tracing::warn!(target: "litegen::hooks", hook = %hook.name, status = ?status, "hook process exited; restarting");
        loop {
            tokio::time::sleep(backoff).await;
            match spawn(&hook, &runner, &shared).await {
                Ok((next, _)) => {
                    child = next;
                    backoff = RESTART_BACKOFF.0;
                    break;
                }
                Err(e) => {
                    tracing::warn!(target: "litegen::hooks", hook = %hook.name, error = %e, "hook restart failed");
                    backoff = (backoff * 2).min(RESTART_BACKOFF.1);
                }
            }
        }
    }
}

async fn write_lines(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(line) = rx.recv().await {
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
}

/// Deliver answers to their calls; when the child's stdout ends, fail the
/// calls that generation still owed.
async fn read_answers(shared: Arc<Shared>, mut lines: Lines<BufReader<ChildStdout>>, generation: u64) {
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Answer>(&line) {
            Ok(answer) => {
                if let Some((_, tx)) = shared.conn.lock().unwrap().pending.remove(&answer.id) {
                    let _ = tx.send(Ok(answer));
                }
            }
            Err(e) => tracing::warn!(target: "litegen::hooks", hook = %shared.name, error = %e, "unreadable answer from hook"),
        }
    }
    let reason = format!("the hook process exited{}", shared.tail());
    let mut conn = shared.conn.lock().unwrap();
    if conn.generation == generation {
        conn.writer = None;
    }
    let owed: Vec<u64> = conn.pending.iter().filter(|(_, (g, _))| *g == generation).map(|(id, _)| *id).collect();
    for id in owed {
        if let Some((_, tx)) = conn.pending.remove(&id) {
            let _ = tx.send(Err(HookCallError::Crashed(reason.clone())));
        }
    }
}

async fn forward_stderr(shared: Arc<Shared>, stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::info!(target: "litegen::hooks", hook = %shared.name, "{line}");
        let mut tail = shared.stderr_tail.lock().unwrap();
        if tail.len() == STDERR_TAIL {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}
