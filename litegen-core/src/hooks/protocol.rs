//! The JSON-lines protocol between litegen and a hook runner
//! (`runners/runner.py`, `runners/runner.mjs`): one `ready` line from the
//! runner, then calls answered in any order, matched by `id`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// litegen → runner.
#[derive(Serialize)]
pub(crate) struct Call<'a> {
    pub id: u64,
    #[serde(rename = "fn")]
    pub function: &'a str,
    pub args: &'a [Value],
}

/// The runner's first line: which hooks the file defines, or why it can't
/// run.
#[derive(Debug, Deserialize)]
pub(crate) struct Ready {
    pub ready: bool,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// runner → litegen: exactly one of `result`, `refuse` or `error` matters.
#[derive(Debug, Deserialize)]
pub(crate) struct Answer {
    pub id: u64,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub refuse: Option<RefuseWire>,
    #[serde(default)]
    pub error: Option<String>,
}

/// A refusal as the runner reports it, before litegen checks it.
#[derive(Debug, Deserialize)]
pub(crate) struct RefuseWire {
    pub status: i64,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}
