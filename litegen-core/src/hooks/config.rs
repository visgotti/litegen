//! The `hooks:` section of `litegen.yaml`, and how each entry resolves to a
//! file, a runtime and a failure policy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The runtime a hook file runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Python,
    Node,
    Bun,
}

impl Runtime {
    /// The executable started for this runtime when the hook names none.
    pub fn default_program(self) -> &'static str {
        match self {
            Runtime::Python => "python3",
            Runtime::Node => "node",
            Runtime::Bun => "bun",
        }
    }

    fn for_extension(ext: &str) -> Option<Runtime> {
        match ext {
            "py" => Some(Runtime::Python),
            "js" | "mjs" | "cjs" => Some(Runtime::Node),
            "ts" | "mts" => Some(Runtime::Bun),
            _ => None,
        }
    }
}

/// What litegen does when a hook crashes, times out or answers nonsense.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OnError {
    /// Answer 503 `hook_failed`.
    Refuse,
    /// Carry on as if the hook had returned nothing.
    Allow,
}

/// One entry of `hooks:`, as written in `litegen.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookConfig {
    /// The hook file, relative to the folder of the config file that
    /// declares it (litegen's working directory, for a `litegen.yaml` found
    /// there) unless absolute.
    pub path: String,
    /// The runtime; by default it follows the file's extension.
    #[serde(default)]
    pub runtime: Option<Runtime>,
    /// The executable to start instead of the runtime's default.
    #[serde(default)]
    pub command: Option<String>,
    /// How long one call may take (default 2000).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Failure policy for both phases. Unset: `pre_call` refuses and
    /// `post_call` lets litegen's response through.
    #[serde(default)]
    pub on_error: Option<OnError>,
    /// Added to the environment the hook inherits from litegen.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A hook entry checked against the disk, with every default applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHook {
    /// The file's name, which errors and logs use to name the hook.
    pub name: String,
    /// The hook file, absolute.
    pub path: PathBuf,
    pub runtime: Runtime,
    /// The executable that runs the hook's runner.
    pub program: String,
    pub timeout: Duration,
    pub pre_policy: OnError,
    pub post_policy: OnError,
    /// `file_access` refuses the file when it fails, unless `on_error: allow`.
    pub file_policy: OnError,
    pub env: BTreeMap<String, String>,
}

/// The time limit when a hook sets none.
pub const DEFAULT_TIMEOUT_MS: u64 = 2000;

impl HookConfig {
    /// Resolve against `base_dir`, the config file's folder: the file must
    /// exist, and its runtime must be named or follow from its extension.
    pub fn resolve(&self, base_dir: &Path) -> Result<ResolvedHook, String> {
        let given = Path::new(&self.path);
        let joined = if given.is_absolute() { given.to_path_buf() } else { base_dir.join(given) };
        let path = std::path::absolute(&joined).unwrap_or(joined);
        if !path.is_file() {
            return Err(format!("hook file not found: {}", path.display()));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| self.path.clone());
        let runtime = match self.runtime {
            Some(r) => r,
            None => path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Runtime::for_extension)
                .ok_or_else(|| format!("cannot tell which runtime runs {name}; set runtime: python, node or bun"))?,
        };
        Ok(ResolvedHook {
            program: self.command.clone().unwrap_or_else(|| runtime.default_program().to_string()),
            timeout: Duration::from_millis(self.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)),
            pre_policy: self.on_error.unwrap_or(OnError::Refuse),
            post_policy: self.on_error.unwrap_or(OnError::Allow),
            file_policy: self.on_error.unwrap_or(OnError::Refuse),
            env: self.env.clone(),
            name,
            path,
            runtime,
        })
    }
}

/// Only the `hooks` key of the config file litegen read (`file`, from
/// [`crate::config::config_file`]), on its own. When the full config fails to
/// load, this is how litegen tells that the failure would drop hooks, and
/// says so. No key is no hooks.
pub fn hooks_in_config_file(file: &crate::config::ConfigFile) -> Result<Vec<HookConfig>, String> {
    file.get::<Vec<HookConfig>>("hooks").map(Option::unwrap_or_default).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn hook(v: serde_json::Value) -> HookConfig {
        serde_json::from_value(v).expect("a valid hook entry")
    }

    fn dir_with(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for f in files {
            let p = dir.path().join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        dir
    }

    #[test]
    fn the_runtime_follows_the_extension_unless_named() {
        let dir = dir_with(&["a.py", "a.mjs", "a.cjs", "a.ts", "a.mts", "a.js"]);
        let runtime = |v| hook(v).resolve(dir.path()).unwrap().runtime;
        assert_eq!(runtime(serde_json::json!({ "path": "a.py" })), Runtime::Python);
        assert_eq!(runtime(serde_json::json!({ "path": "a.mjs" })), Runtime::Node);
        assert_eq!(runtime(serde_json::json!({ "path": "a.cjs" })), Runtime::Node);
        assert_eq!(runtime(serde_json::json!({ "path": "a.js" })), Runtime::Node);
        assert_eq!(runtime(serde_json::json!({ "path": "a.ts" })), Runtime::Bun);
        assert_eq!(runtime(serde_json::json!({ "path": "a.mts" })), Runtime::Bun);
        assert_eq!(runtime(serde_json::json!({ "path": "a.js", "runtime": "bun" })), Runtime::Bun);
        assert_eq!(runtime(serde_json::json!({ "path": "a.ts", "runtime": "node" })), Runtime::Node);
    }

    #[test]
    fn an_unknown_extension_without_a_runtime_is_an_error() {
        let dir = dir_with(&["a.rb"]);
        let err = hook(serde_json::json!({ "path": "a.rb" })).resolve(dir.path()).unwrap_err();
        assert_eq!(err, "cannot tell which runtime runs a.rb; set runtime: python, node or bun");
        // Naming the runtime settles it.
        let ok = hook(serde_json::json!({ "path": "a.rb", "runtime": "python" })).resolve(dir.path()).unwrap();
        assert_eq!(ok.runtime, Runtime::Python);
    }

    #[test]
    fn the_path_resolves_against_the_base_dir_and_must_exist() {
        let dir = dir_with(&["hooks/auth.py"]);
        let r = hook(serde_json::json!({ "path": "hooks/auth.py" })).resolve(dir.path()).unwrap();
        assert_eq!(r.path, dir.path().join("hooks").join("auth.py"));
        assert!(r.path.is_absolute());
        assert_eq!(r.name, "auth.py");

        let missing = dir.path().join("hooks").join("gone.py");
        let err = hook(serde_json::json!({ "path": "hooks/gone.py" })).resolve(dir.path()).unwrap_err();
        assert_eq!(err, format!("hook file not found: {}", missing.display()));

        // An absolute path is taken as is.
        let abs = dir.path().join("hooks").join("auth.py");
        let r = hook(serde_json::json!({ "path": abs.display().to_string() })).resolve(std::path::Path::new("/nowhere")).unwrap();
        assert_eq!(r.path, abs);
    }

    #[test]
    fn defaults_are_2000_ms_refuse_before_allow_after() {
        let dir = dir_with(&["a.py"]);
        let r = hook(serde_json::json!({ "path": "a.py" })).resolve(dir.path()).unwrap();
        assert_eq!(r.timeout, Duration::from_millis(2000));
        assert_eq!(r.pre_policy, OnError::Refuse);
        assert_eq!(r.post_policy, OnError::Allow);
        assert_eq!(r.file_policy, OnError::Refuse, "a failing file check closes the file");

        let allow = hook(serde_json::json!({ "path": "a.py", "on_error": "allow", "timeout_ms": 150 })).resolve(dir.path()).unwrap();
        assert_eq!((allow.pre_policy, allow.post_policy, allow.file_policy), (OnError::Allow, OnError::Allow, OnError::Allow));
        assert_eq!(allow.timeout, Duration::from_millis(150));

        let refuse = hook(serde_json::json!({ "path": "a.py", "on_error": "refuse" })).resolve(dir.path()).unwrap();
        assert_eq!((refuse.pre_policy, refuse.post_policy), (OnError::Refuse, OnError::Refuse));
    }

    #[test]
    fn command_overrides_the_program() {
        let dir = dir_with(&["a.py", "a.mjs", "a.ts"]);
        let program = |v| hook(v).resolve(dir.path()).unwrap().program;
        assert_eq!(program(serde_json::json!({ "path": "a.py" })), "python3");
        assert_eq!(program(serde_json::json!({ "path": "a.mjs" })), "node");
        assert_eq!(program(serde_json::json!({ "path": "a.ts" })), "bun");
        assert_eq!(program(serde_json::json!({ "path": "a.py", "command": "/opt/venv/bin/python" })), "/opt/venv/bin/python");

        let env = hook(serde_json::json!({ "path": "a.py", "env": { "JWKS_URL": "https://id.example/jwks" } }))
            .resolve(dir.path())
            .unwrap()
            .env;
        assert_eq!(env.get("JWKS_URL").map(String::as_str), Some("https://id.example/jwks"));
    }

    #[test]
    fn an_unknown_key_in_a_hook_entry_is_refused() {
        // A typo such as `timout_ms` must not be dropped silently: hooks fail
        // fast rather than run with a setting the operator didn't get.
        let err = serde_json::from_value::<HookConfig>(serde_json::json!({ "path": "a.py", "timout_ms": 50 })).unwrap_err();
        assert!(err.to_string().contains("timout_ms"), "{err}");
    }

    /// The file litegen would read in `dir`, with no arguments or variables.
    fn file_in(dir: &Path) -> Option<crate::config::ConfigFile> {
        crate::config::config_file(dir, None, &std::collections::HashMap::new()).unwrap()
    }

    #[test]
    fn hooks_in_config_file_reads_only_hooks() {
        let dir = tempfile::tempdir().unwrap();
        // No file at all: nothing to read hooks from.
        assert!(file_in(dir.path()).is_none());

        // A value the full config rejects (a number given as text), beside
        // valid hooks.
        std::fs::write(
            dir.path().join("litegen.yaml"),
            "cost_markup_percent: lots\nhooks:\n  - path: hooks/auth.py\n  - path: hooks/tag.mjs\n    timeout_ms: 500\n",
        )
        .unwrap();
        let hooks = hooks_in_config_file(&file_in(dir.path()).unwrap()).unwrap();
        assert_eq!(hooks.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), ["hooks/auth.py", "hooks/tag.mjs"]);
        assert_eq!(hooks[1].timeout_ms, Some(500));

        // A file without the key: no hooks.
        std::fs::write(dir.path().join("litegen.yaml"), "server:\n  port: 1\n").unwrap();
        assert!(hooks_in_config_file(&file_in(dir.path()).unwrap()).unwrap().is_empty());

        // A malformed `hooks:` is an error, not "no hooks".
        std::fs::write(dir.path().join("litegen.yaml"), "hooks:\n  - path: a.py\n    runtime: cobol\n").unwrap();
        let err = hooks_in_config_file(&file_in(dir.path()).unwrap()).unwrap_err();
        assert!(err.contains("cobol"), "{err}");
    }

    #[test]
    fn hooks_are_read_from_the_file_main_resolved() {
        // A file named with --config, beside a litegen.yaml that declares none.
        let dir = dir_with(&["litegen.yaml"]);
        std::fs::create_dir(dir.path().join("conf")).unwrap();
        std::fs::write(dir.path().join("conf/site.yaml"), "hooks:\n  - path: hooks/auth.py\n").unwrap();
        let file = crate::config::config_file(dir.path(), Some(Path::new("conf/site.yaml")), &std::collections::HashMap::new())
            .unwrap()
            .expect("the named file");
        let hooks = hooks_in_config_file(&file).unwrap();
        assert_eq!(hooks.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(), ["hooks/auth.py"]);
    }
}
