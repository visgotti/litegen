//! Generation call hooks: operator code, in Python, Node or Bun, run around
//! litegen's generation routes. `pre_call` sees the raw request before
//! authentication and may change or refuse it; `post_call` sees the finished
//! response and may change it. Each hook file runs in its own long-lived
//! child process (see `docs/hooks.md`).

pub mod config;
pub mod layer;
pub mod process;
mod protocol;
mod request;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub use config::{hooks_in_config_file, HookConfig, OnError, ResolvedHook, Runtime};
pub use layer::{file_access, post_call_layer, pre_call_layer};

use process::{HookFn, HookProcess};

/// The configured hooks, running, in the order `litegen.yaml` lists them.
pub struct HookSet {
    hooks: Vec<Hook>,
    /// Requests with a body the `pre_call` layer holds at once, counted once
    /// the body has arrived; past that it answers 503 `server_at_capacity`,
    /// as litegen's own backpressure does. A request with no body the layer
    /// reads (a GET: a status read, a file) never takes one.
    pub(crate) buffering: Arc<tokio::sync::Semaphore>,
    /// The same for the `post_call` layer, from a pool of its own so one
    /// request never needs two permits from one pool.
    pub(crate) post_buffering: Arc<tokio::sync::Semaphore>,
    /// The bytes the hook layers may hold of bodies they read, and how long
    /// a body may take to arrive (`api::middleware::body`). A body read here
    /// is not charged again by the routes behind.
    pub(crate) body: crate::api::middleware::body::BodyBudget,
}

pub(crate) struct Hook {
    pub name: String,
    pub process: Arc<HookProcess>,
    pub timeout: Duration,
    pub pre_policy: OnError,
    pub post_policy: OnError,
    pub file_policy: OnError,
}

impl HookSet {
    /// Start every hook, or fail naming the first that can't start. No hooks
    /// configured is `None`. `max_buffered` bounds the bodies the hook layer
    /// holds at once (litegen passes `backpressure.max_in_flight`), and each
    /// body gets `body_timeout` to arrive (`server.request_body_timeout_secs`).
    pub async fn start(
        configs: &[HookConfig],
        base_dir: &Path,
        max_buffered: usize,
        body_timeout: Duration,
    ) -> Result<Option<Arc<HookSet>>, String> {
        if configs.is_empty() {
            return Ok(None);
        }
        let mut hooks = Vec::with_capacity(configs.len());
        for config in configs {
            let resolved = config.resolve(base_dir).map_err(|e| format!("hook {}: {e}", config.path))?;
            let process = HookProcess::start(&resolved).await?;
            hooks.push(Hook {
                name: resolved.name.clone(),
                process,
                timeout: resolved.timeout,
                pre_policy: resolved.pre_policy,
                post_policy: resolved.post_policy,
                file_policy: resolved.file_policy,
            });
        }
        let permits = max_buffered.max(1);
        Ok(Some(Arc::new(HookSet {
            hooks,
            buffering: Arc::new(tokio::sync::Semaphore::new(permits)),
            post_buffering: Arc::new(tokio::sync::Semaphore::new(permits)),
            body: crate::api::middleware::body::BodyBudget::new(permits, request::MAX_BODY_BYTES, body_timeout),
        })))
    }

    /// The running hooks' file names, in order.
    pub fn names(&self) -> Vec<&str> {
        self.hooks.iter().map(|h| h.name.as_str()).collect()
    }

    pub(crate) fn defines(&self, f: HookFn) -> bool {
        self.hooks.iter().any(|h| h.process.defines(f))
    }

    pub(crate) fn with(&self, f: HookFn) -> impl Iterator<Item = &Hook> {
        self.hooks.iter().filter(move |h| h.process.defines(f))
    }
}
