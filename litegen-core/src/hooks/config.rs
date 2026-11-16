//! The `hooks:` section of `litegen.yaml`, and how each entry resolves to a
//! file, a runtime and a failure policy.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A group of hooked routes, as a hook entry's `routes:` names it. Each
/// hooked route is in exactly one group, given where the router is built
/// (`api::handlers::create_router_with_hooks`); `docs/hooks.md` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RouteGroup {
    /// `POST /v1/images/generations`, `/v1/images/segmentations`,
    /// `/v1/videos/generations` and `/v1/models3d/generations`.
    Generations,
    /// The price checks: `POST /v1/images/cost`, `/v1/videos/cost` and
    /// `/v1/models3d/cost`.
    Cost,
    /// The job status reads: `GET /v1/videos/{id}` and `/v1/models3d/{id}`.
    Status,
    /// `POST /v1/utils/models3d/convert`.
    Convert,
    /// The stored-file route, `GET /v1/generations/{id}/assets/{index}`.
    Files,
}

impl RouteGroup {
    /// Every group: where a hook entry without `routes:` runs.
    pub const ALL: [RouteGroup; 5] = [RouteGroup::Generations, RouteGroup::Cost, RouteGroup::Status, RouteGroup::Convert, RouteGroup::Files];

    /// The group's name in `routes:`.
    pub fn name(self) -> &'static str {
        match self {
            RouteGroup::Generations => "generations",
            RouteGroup::Cost => "cost",
            RouteGroup::Status => "status",
            RouteGroup::Convert => "convert",
            RouteGroup::Files => "files",
        }
    }
}

impl Serialize for RouteGroup {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for RouteGroup {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        RouteGroup::ALL.into_iter().find(|g| g.name() == name).ok_or_else(|| {
            let groups = RouteGroup::ALL.map(RouteGroup::name).join(", ");
            serde::de::Error::custom(format!("routes: no route group is named {name:?}; the groups are {groups}"))
        })
    }
}

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
    /// The route groups whose requests the hook's `pre_call` and `post_call`
    /// see. Unset: every hooked route.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "route_groups")]
    pub routes: Option<Vec<RouteGroup>>,
}

/// `routes:` names at least one route group, each once: a hook that runs on
/// every hooked route leaves the key out.
fn route_groups<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<RouteGroup>>, D::Error> {
    let groups = Option::<Vec<RouteGroup>>::deserialize(d)?.unwrap_or_default();
    if groups.is_empty() {
        return Err(serde::de::Error::custom("routes: name at least one route group, or leave routes out to hook every route"));
    }
    if let Some(twice) = groups.iter().enumerate().find_map(|(i, g)| groups[..i].contains(g).then_some(g)) {
        return Err(serde::de::Error::custom(format!("routes: {:?} is named twice", twice.name())));
    }
    Ok(Some(groups))
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
    /// The route groups whose requests reach `pre_call` and `post_call`.
    pub routes: BTreeSet<RouteGroup>,
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
            routes: self.routes.as_deref().unwrap_or(&RouteGroup::ALL).iter().copied().collect(),
            name,
            path,
            runtime,
        })
    }
}

/// How errors and warnings name the entry at `index` of `hooks:`: by its
/// place in the list, counted from 0 as config-rs counts (`hooks[1]`), and
/// its `path` as written, when it has one.
pub(crate) fn entry_name(index: usize, path: Option<&str>) -> String {
    match path {
        Some(path) => format!("hooks[{index}] ({path})"),
        None => format!("hooks[{index}]"),
    }
}

/// The entry at `index` of `hooks:`, as config-rs read it, or the error that
/// refuses it, naming the entry. config-rs adds the key only to its own type
/// errors, so a message from litegen's checks or from serde (an unknown key)
/// would name nothing: each entry is read on its own and its error named.
fn entry(index: usize, value: ::config::Value) -> Result<HookConfig, String> {
    value.clone().try_deserialize::<HookConfig>().map_err(|e| {
        let path = value.into_table().ok().and_then(|t| t.get("path").cloned()).and_then(|p| p.into_string().ok());
        format!("{}: {e}", entry_name(index, path.as_deref()))
    })
}

/// `hooks:` as [`crate::config::AppConfig`] reads it (its `deserialize_with`):
/// each entry on its own, so that its error names it.
pub(crate) fn hook_entries<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<HookConfig>, D::Error> {
    let entries = Vec::<::config::Value>::deserialize(d)?;
    entries.into_iter().enumerate().map(|(i, v)| entry(i, v).map_err(serde::de::Error::custom)).collect()
}

/// Only the `hooks` key of the config file litegen read (`file`, from
/// [`crate::config::config_file`]), on its own. When the full config fails to
/// load, this is how litegen tells that the failure would drop hooks, and
/// says so. No key is no hooks. An entry's error names the entry.
pub fn hooks_in_config_file(file: &crate::config::ConfigFile) -> Result<Vec<HookConfig>, String> {
    let entries = file.get::<Vec<::config::Value>>("hooks").map_err(|e| e.to_string())?.unwrap_or_default();
    entries.into_iter().enumerate().map(|(i, v)| entry(i, v)).collect()
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

    /// A hook entry's `routes:`, as written, or the error that refuses it.
    fn routes_of(routes: serde_json::Value) -> Result<Option<Vec<RouteGroup>>, String> {
        serde_json::from_value::<HookConfig>(serde_json::json!({ "path": "a.py", "routes": routes }))
            .map(|h| h.routes)
            .map_err(|e| e.to_string())
    }

    #[test]
    fn routes_names_route_groups() {
        assert_eq!(routes_of(serde_json::json!(["generations", "status"])), Ok(Some(vec![RouteGroup::Generations, RouteGroup::Status])));
        assert_eq!(routes_of(serde_json::json!(["generations", "cost", "status", "convert", "files"])), Ok(Some(RouteGroup::ALL.to_vec())));
    }

    #[test]
    fn an_unknown_route_group_is_refused_naming_the_groups() {
        assert_eq!(
            routes_of(serde_json::json!(["generations", "costs"])),
            Err(r#"routes: no route group is named "costs"; the groups are generations, cost, status, convert, files"#.to_string())
        );
    }

    const NO_ROUTES: &str = "routes: name at least one route group, or leave routes out to hook every route";

    #[test]
    fn an_empty_routes_list_is_refused() {
        assert_eq!(routes_of(serde_json::json!([])), Err(NO_ROUTES.to_string()));
        // `routes:` with nothing after it, in YAML.
        assert_eq!(routes_of(serde_json::Value::Null), Err(NO_ROUTES.to_string()));
    }

    #[test]
    fn a_hook_runs_on_every_route_group_unless_routes_names_fewer() {
        let dir = dir_with(&["a.py"]);
        let routes = |v| hook(v).resolve(dir.path()).unwrap().routes;
        assert_eq!(routes(serde_json::json!({ "path": "a.py" })), std::collections::BTreeSet::from(RouteGroup::ALL));
        assert_eq!(
            routes(serde_json::json!({ "path": "a.py", "routes": ["status", "generations"] })),
            std::collections::BTreeSet::from([RouteGroup::Generations, RouteGroup::Status])
        );
    }

    #[test]
    fn a_route_group_named_twice_is_refused() {
        assert_eq!(routes_of(serde_json::json!(["cost", "generations", "cost"])), Err(r#"routes: "cost" is named twice"#.to_string()));
    }

    /// The routes of each hook in `yaml`, loaded as litegen starts in a
    /// folder holding that `litegen.yaml`, or the error that stops it.
    fn routes_in_litegen_yaml(yaml: &str) -> Result<Vec<Option<Vec<RouteGroup>>>, String> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("litegen.yaml"), yaml).unwrap();
        crate::config::load_config_from(dir.path(), None, &std::collections::HashMap::new())
            .map(|c| c.hooks.into_iter().map(|h| h.routes).collect())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn litegen_does_not_start_on_an_empty_routes_list() {
        let refused = format!("hooks[0] (hooks/auth.py): {NO_ROUTES}");
        assert_eq!(routes_in_litegen_yaml("hooks:\n  - path: hooks/auth.py\n    routes: []\n"), Err(refused.clone()));
        assert_eq!(routes_in_litegen_yaml("hooks:\n  - path: hooks/auth.py\n    routes:\n"), Err(refused));
    }

    /// config-rs adds the key only to its own type errors, so a message that
    /// litegen or serde writes about a hook entry named no entry. Each names
    /// it now, by its place in `hooks:` (from 0, as config-rs counts) and its
    /// path as written.
    #[test]
    fn a_hook_entrys_error_names_the_entry() {
        let second = |lines: &str| routes_in_litegen_yaml(&format!("hooks:\n  - path: hooks/auth.py\n  - path: hooks/tag.mjs\n{lines}")).unwrap_err();
        // litegen's own checks.
        assert_eq!(second("    routes: [cost, cost]\n"), r#"hooks[1] (hooks/tag.mjs): routes: "cost" is named twice"#);
        assert_eq!(
            second("    routes: [costs]\n"),
            r#"hooks[1] (hooks/tag.mjs): routes: no route group is named "costs"; the groups are generations, cost, status, convert, files"#
        );
        // serde's (an unknown key) and config-rs's (an unknown runtime, a
        // number that isn't one), each in its own words after the entry.
        for (lines, says) in [("    timout_ms: 50\n", "unknown field `timout_ms`"), ("    runtime: cobol\n", "cobol"), ("    timeout_ms: lots\n", "`timeout_ms`")] {
            let err = second(lines);
            assert!(err.starts_with("hooks[1] (hooks/tag.mjs): ") && err.contains(says), "{lines}: {err}");
        }
        // An entry with no path is named by its place alone.
        assert_eq!(routes_in_litegen_yaml("hooks:\n  - path: hooks/auth.py\n  - runtime: node\n"), Err("hooks[1]: missing field `path`".to_string()));
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

    /// When the config doesn't load, litegen reads `hooks:` again on its own,
    /// to say the hooks would be dropped (main's `refuse_to_start`): that
    /// error names the entry too.
    #[test]
    fn hooks_in_config_file_names_the_entry_it_refuses() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("litegen.yaml"), "hooks:\n  - path: hooks/auth.py\n  - path: hooks/tag.mjs\n    routes: [cost, cost]\n").unwrap();
        let err = hooks_in_config_file(&file_in(dir.path()).unwrap()).unwrap_err();
        assert_eq!(err, r#"hooks[1] (hooks/tag.mjs): routes: "cost" is named twice"#);
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
