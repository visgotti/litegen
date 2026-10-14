use std::collections::HashMap;
use std::path::Path;

use crate::capabilities::loader::{self, LoadError};
use crate::capabilities::schema::*;

#[derive(Debug, Default)]
pub struct CapabilityRegistry {
    models: HashMap<String, ModelSchema>,
    by_provider: HashMap<String, Vec<String>>,
    /// Models created through the management API, layered over the catalog.
    /// `get`/`all`/`for_provider` stay catalog-only; serving paths use
    /// [`resolve`](Self::resolve) and [`served`](Self::served).
    runtime: std::sync::RwLock<HashMap<String, ModelSchema>>,
    /// Model ids switched off platform-wide (catalog or runtime).
    disabled: std::sync::RwLock<std::collections::HashSet<String>>,
}

impl CapabilityRegistry {
    pub fn from_dir(dir: &Path) -> Result<Self, LoadError> {
        let files = loader::discover_model_files(dir)?;
        let mut inputs: Vec<(String, String)> = Vec::new();
        for p in files {
            let content = std::fs::read_to_string(&p).map_err(|e| LoadError::Io {
                path: p.display().to_string(),
                source: e,
            })?;
            inputs.push((p.display().to_string(), content));
        }
        let refs: Vec<(&str, &str)> = inputs.iter()
            .map(|(p, c)| (p.as_str(), c.as_str()))
            .collect();
        Self::from_yaml_strs(&refs)
    }

    pub fn from_yaml_strs(files: &[(&str, &str)]) -> Result<Self, LoadError> {
        let mut all = Vec::new();
        let mut errors: Vec<LoadError> = Vec::new();
        for (path, yaml) in files {
            match loader::parse_file(path, yaml) {
                Ok(models) => {
                    for m in models { all.push((path.to_string(), m)); }
                }
                Err(e) => errors.push(e),
            }
        }
        if !errors.is_empty() {
            return Err(if errors.len() == 1 {
                errors.remove(0)
            } else {
                LoadError::Aggregate(errors)
            });
        }

        let mut models: HashMap<String, ModelSchema> = HashMap::new();
        let mut by_provider: HashMap<String, Vec<String>> = HashMap::new();
        for (path, m) in all {
            if models.contains_key(&m.id) {
                return Err(LoadError::Validate {
                    path,
                    message: format!("duplicate model id '{}'", m.id),
                });
            }
            by_provider.entry(m.provider.clone()).or_default().push(m.id.clone());
            models.insert(m.id.clone(), m);
        }
        for ids in by_provider.values_mut() { ids.sort(); }
        Ok(Self { models, by_provider, ..Default::default() })
    }

    pub fn get(&self, id: &str) -> Option<&ModelSchema> { self.models.get(id) }
    pub fn all(&self) -> impl Iterator<Item = &ModelSchema> { self.models.values() }
    pub fn for_provider<'a>(&'a self, provider: &str) -> impl Iterator<Item = &'a ModelSchema> {
        let ids = self.by_provider.get(provider).cloned().unwrap_or_default();
        ids.into_iter().filter_map(move |id| self.models.get(&id))
    }
    pub fn len(&self) -> usize { self.models.len() }
    pub fn is_empty(&self) -> bool { self.models.is_empty() }

    // ─── Runtime layer (management API) ─────────────────────────────────

    /// The schema served for `id`: catalog first, then runtime. None when the
    /// id is unknown or disabled.
    pub fn resolve(&self, id: &str) -> Option<ModelSchema> {
        if self.is_disabled(id) {
            return None;
        }
        self.models.get(id).cloned().or_else(|| self.runtime_model(id))
    }

    /// Every served model (catalog + runtime, minus disabled).
    pub fn served(&self) -> Vec<ModelSchema> {
        let disabled = self.disabled.read().unwrap_or_else(|p| p.into_inner());
        let runtime = self.runtime.read().unwrap_or_else(|p| p.into_inner());
        self.models
            .values()
            .chain(runtime.values())
            .filter(|m| !disabled.contains(&m.id))
            .cloned()
            .collect()
    }

    /// Whether `id` is defined by the YAML catalog.
    pub fn is_catalog(&self, id: &str) -> bool {
        self.models.contains_key(id)
    }

    pub fn runtime_model(&self, id: &str) -> Option<ModelSchema> {
        self.runtime.read().unwrap_or_else(|p| p.into_inner()).get(id).cloned()
    }

    pub fn runtime_models(&self) -> Vec<ModelSchema> {
        let mut v: Vec<ModelSchema> = self.runtime.read().unwrap_or_else(|p| p.into_inner()).values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Add or replace a runtime model. Callers validate it first and must
    /// never pass a catalog id.
    pub fn upsert_runtime(&self, schema: ModelSchema) {
        self.runtime.write().unwrap_or_else(|p| p.into_inner()).insert(schema.id.clone(), schema);
    }

    pub fn remove_runtime(&self, id: &str) -> bool {
        self.runtime.write().unwrap_or_else(|p| p.into_inner()).remove(id).is_some()
    }

    pub fn set_disabled(&self, id: &str, disabled: bool) {
        let mut set = self.disabled.write().unwrap_or_else(|p| p.into_inner());
        if disabled {
            set.insert(id.to_string());
        } else {
            set.remove(id);
        }
    }

    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.read().unwrap_or_else(|p| p.into_inner()).contains(id)
    }
}

#[cfg(test)]
mod ship_tests {
    use super::*;

    fn shipped_registry() -> CapabilityRegistry {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        CapabilityRegistry::from_dir(&p).expect("models/*.yaml must load cleanly")
    }

    /// Smoke test: every shipped models/*.yaml must load cleanly.
    #[test]
    fn ships_models_directory_loads() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        if !p.exists() {
            return; // smoke test only runs when models/ is present
        }
        let r = CapabilityRegistry::from_dir(&p)
            .expect("models/*.yaml must load cleanly");
        assert!(r.len() >= 20, "expected at least 20 shipped models, got {}", r.len());
    }

    /// Every new mock model id must resolve in the registry.
    #[test]
    fn new_mock_model_ids_present() {
        let r = shipped_registry();
        let new_ids = [
            "mock/all-params-image",
            "mock/freeform-size-image",
            "mock/url-refs-image",
            "mock/base64-refs-image",
            "mock/multipart-refs-image",
            "mock/inpainting-image",
            "mock/passthrough-image",
            "mock/expensive-image",
            "mock/keyframe-video",
            "mock/passthrough-video",
            "mock/strict-duration-video",
            "mock/expensive-video",
        ];
        for id in &new_ids {
            assert!(
                r.get(id).is_some(),
                "model '{}' missing from registry",
                id
            );
        }
    }
}
