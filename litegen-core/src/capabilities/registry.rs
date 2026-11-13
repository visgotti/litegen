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
    runtime: std::sync::RwLock<Runtime>,
    /// Model ids switched off platform-wide (catalog or runtime).
    disabled: std::sync::RwLock<Disabled>,
}

/// The runtime layer, and how many times it has changed.
#[derive(Debug, Default)]
struct Runtime {
    models: HashMap<String, ModelSchema>,
    /// Bumped by every `upsert_runtime` and `remove_runtime`, and by an
    /// update that changed the layer (see
    /// [`CapabilityRegistry::update_runtime_unless_changed`]).
    version: u64,
}

/// The disabled set, and how many times it has changed.
#[derive(Debug, Default)]
struct Disabled {
    ids: std::collections::HashSet<String>,
    /// Bumped by every `set_disabled`, and by a replace that changed the set
    /// (see [`CapabilityRegistry::replace_disabled_unless_changed`]).
    version: u64,
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
        // Past its vendor's shutdown, a model is no longer served (L14).
        self.models
            .get(id)
            .cloned()
            .or_else(|| self.runtime_model(id))
            .filter(|m| m.status() != ModelStatus::Retired)
    }

    /// The row for `id` when it is past its retirement (catalog or runtime):
    /// what a request for it is told instead of "not found".
    pub fn retired(&self, id: &str) -> Option<ModelSchema> {
        self.models
            .get(id)
            .cloned()
            .or_else(|| self.runtime_model(id))
            .filter(|m| m.status() == ModelStatus::Retired)
    }

    /// A fingerprint of everything served: it changes whenever any served
    /// model's field does (L14), so a client can cache the listing by it.
    pub fn catalog_version(&self) -> String {
        use sha2::Digest;
        let mut served = self.served();
        served.sort_by(|a, b| a.id.cmp(&b.id));
        // Over a key-sorted form: several fields are hash maps, whose order
        // differs between processes, and so between replicas.
        let bytes = serde_json::to_vec(&canonical(serde_json::to_value(&served).unwrap_or_default())).unwrap_or_default();
        let digest = sha2::Sha256::digest(&bytes);
        digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
    }

    /// Every served model (catalog + runtime, minus disabled).
    pub fn served(&self) -> Vec<ModelSchema> {
        let disabled = self.disabled.read().unwrap_or_else(|p| p.into_inner());
        let runtime = self.runtime.read().unwrap_or_else(|p| p.into_inner());
        self.models
            .values()
            .chain(runtime.models.values())
            .filter(|m| !disabled.ids.contains(&m.id) && m.status() != ModelStatus::Retired)
            .cloned()
            .collect()
    }

    /// Whether `id` is defined by the YAML catalog.
    pub fn is_catalog(&self, id: &str) -> bool {
        self.models.contains_key(id)
    }

    pub fn runtime_model(&self, id: &str) -> Option<ModelSchema> {
        self.runtime.read().unwrap_or_else(|p| p.into_inner()).models.get(id).cloned()
    }

    pub fn runtime_models(&self) -> Vec<ModelSchema> {
        let mut v: Vec<ModelSchema> = self.runtime.read().unwrap_or_else(|p| p.into_inner()).models.values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Add or replace a runtime model. Callers validate it first and must
    /// never pass a catalog id.
    pub fn upsert_runtime(&self, schema: ModelSchema) {
        let mut runtime = self.runtime.write().unwrap_or_else(|p| p.into_inner());
        runtime.models.insert(schema.id.clone(), schema);
        runtime.version += 1;
    }

    pub fn remove_runtime(&self, id: &str) -> bool {
        let mut runtime = self.runtime.write().unwrap_or_else(|p| p.into_inner());
        runtime.version += 1;
        runtime.models.remove(id).is_some()
    }

    /// How many times the runtime layer has changed. A refresh reads it before
    /// the database, and applies what it read with
    /// [`update_runtime_unless_changed`](Self::update_runtime_unless_changed).
    pub fn runtime_version(&self) -> u64 {
        self.runtime.read().unwrap_or_else(|p| p.into_inner()).version
    }

    /// Add `upserts` to the runtime layer and take `removals` out, all at
    /// once, unless the layer changed since `version` was read: then nothing
    /// is applied and the answer is `None`. A save on this replica that landed
    /// while the caller read the database therefore stands, and the caller's
    /// next read (which holds the save) applies. Otherwise whether anything
    /// changed: any upsert, or a removal of a model that was there. A change
    /// moves the version, so a second update planned from the same version
    /// stands down too.
    pub fn update_runtime_unless_changed(&self, version: u64, upserts: Vec<ModelSchema>, removals: &[String]) -> Option<bool> {
        let mut runtime = self.runtime.write().unwrap_or_else(|p| p.into_inner());
        if runtime.version != version {
            return None;
        }
        let mut changed = !upserts.is_empty();
        for m in upserts {
            runtime.models.insert(m.id.clone(), m);
        }
        for id in removals {
            changed |= runtime.models.remove(id).is_some();
        }
        if changed {
            runtime.version += 1;
        }
        Some(changed)
    }

    pub fn set_disabled(&self, id: &str, disabled: bool) {
        let mut set = self.disabled.write().unwrap_or_else(|p| p.into_inner());
        if disabled {
            set.ids.insert(id.to_string());
        } else {
            set.ids.remove(id);
        }
        set.version += 1;
    }

    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.read().unwrap_or_else(|p| p.into_inner()).ids.contains(id)
    }

    /// Every disabled model id, sorted.
    pub fn disabled_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.disabled.read().unwrap_or_else(|p| p.into_inner()).ids.iter().cloned().collect();
        ids.sort();
        ids
    }

    /// How many times the disabled set has changed. A refresh reads it before
    /// the database, and applies what it read with
    /// [`replace_disabled_unless_changed`](Self::replace_disabled_unless_changed).
    pub fn disabled_version(&self) -> u64 {
        self.disabled.read().unwrap_or_else(|p| p.into_inner()).version
    }

    /// Make `ids` the disabled set, unless it changed since `version` was
    /// read: then nothing is applied and the answer is `None`. A PATCH on
    /// this replica that landed while the caller read the database therefore
    /// stands, and the caller's next read (which holds it) applies. Otherwise
    /// whether the set changed. A change moves the version (one that changes
    /// nothing leaves it), so a second replace planned from the same version
    /// stands down too.
    pub fn replace_disabled_unless_changed(&self, version: u64, ids: std::collections::HashSet<String>) -> Option<bool> {
        let mut set = self.disabled.write().unwrap_or_else(|p| p.into_inner());
        if set.version != version {
            return None;
        }
        if set.ids == ids {
            return Some(false);
        }
        set.ids = ids;
        set.version += 1;
        Some(true)
    }
}

/// `v` with every object's keys in order, at every depth.
fn canonical(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(entries.into_iter().map(|(k, v)| (k, canonical(v))).collect())
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
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

    /// Vendors that take an fps from a list say so in the catalog, and the
    /// validator holds callers to it.
    #[test]
    fn ltx_2_3_and_hunyuan_i2v_state_their_fps_sets() {
        let r = shipped_registry();
        for (id, want) in [("fal/ltx-2.3", vec![24, 25, 48, 50]), ("hunyuan/hunyuan-video-i2v", vec![16, 24, 30])] {
            match r.get(id).and_then(|m| m.params.get("fps")) {
                Some(ParamSpec::Int(s)) => assert_eq!(s.values, want, "{id}"),
                other => panic!("{id}: fps is {other:?}"),
            }
        }
    }

    /// The rows that took their start frame only as `init` take
    /// `first_frame` too (and make it the default), so the API reports
    /// `supports_first_frame` for them; one image still at most.
    #[test]
    fn the_init_start_frame_rows_take_first_frame_too() {
        let r = shipped_registry();
        for id in ["fal/video", "fal/ltx-2.3", "runway/gen4-turbo", "runway/gen4.5"] {
            let refs = r.get(id).and_then(|m| m.ref_inputs.as_ref()).unwrap_or_else(|| panic!("{id} takes refs"));
            assert!(refs.roles.contains_key("first_frame") && refs.roles.contains_key("init"), "{id}: {:?}", refs.roles.keys());
            assert_eq!(refs.default_role.as_deref(), Some("first_frame"), "{id}");
            assert_eq!(refs.max_total, 1, "{id}");
        }
    }

    /// Rows whose vendor's price varies price the variant asked for, as the
    /// vendors' tables say (re-read 2026-11-03; each row cites its source).
    #[test]
    fn varying_rows_price_the_variant_asked_for() {
        let r = shipped_registry();
        let quote = |id: &str, params: serde_json::Value, refs: serde_json::Value| -> f64 {
            let m = r.get(id).unwrap_or_else(|| panic!("{id}"));
            let base: crate::types::BaseGenerationRequest =
                serde_json::from_value(serde_json::json!({ "model": id, "prompt": "p", "reference_images": refs })).unwrap();
            crate::capabilities::pricing::request_price_usd(m, &base, &params)
        };
        let none = serde_json::json!([]);
        // `n` reference images; the row's default role takes each.
        let refs = |n: usize| serde_json::Value::Array((0..n).map(|i| serde_json::json!({ "type": "url", "value": format!("https://x/{i}.png") })).collect());
        let cases: Vec<(&str, serde_json::Value, serde_json::Value, f64)> = vec![
            // Seedance 1.0 Pro: $2.5 per 1M tokens, 24 fps, 16:9 sizes.
            ("bytedance/doubao-seedance-1-0-pro-250528", serde_json::json!({ "duration_seconds": 10.0 }), none.clone(), 1.224),
            ("bytedance/doubao-seedance-1-0-pro-250528", serde_json::json!({ "resolution": "720p" }), none.clone(), 0.2574),
            ("bytedance/doubao-seedance-1-0-pro-250528", serde_json::json!({ "resolution": "480p" }), none.clone(), 0.1215),
            // Seedance 1.0 Pro Fast: $1 per 1M tokens.
            ("bytedance/seedance-1-0-pro-fast-251015", serde_json::json!({}), none.clone(), 0.2448),
            ("bytedance/seedance-1-0-pro-fast-251015", serde_json::json!({ "resolution": "720p", "duration_seconds": 10.0 }), none.clone(), 0.20592),
            // Seedance 2.0 mini: $3.5 per 1M tokens, 720p default.
            ("bytedance/dreamina-seedance-2-0-mini-260615", serde_json::json!({}), none.clone(), 0.378),
            ("bytedance/dreamina-seedance-2-0-mini-260615", serde_json::json!({ "resolution": "480p" }), none.clone(), 0.17577),
            // MiniMax Hailuo: 768P 6 s, 768P 10 s, 1080P 6 s.
            ("minimax/MiniMax-Hailuo-02", serde_json::json!({ "duration_seconds": 10.0 }), none.clone(), 0.56),
            ("minimax/MiniMax-Hailuo-02", serde_json::json!({ "resolution": "1080P" }), none.clone(), 0.49),
            ("minimax/MiniMax-Hailuo-2.3", serde_json::json!({ "duration_seconds": 10.0 }), none.clone(), 0.56),
            ("minimax/MiniMax-Hailuo-2.3", serde_json::json!({ "resolution": "1080P" }), none.clone(), 0.49),
            ("minimax/MiniMax-Hailuo-2.3-Fast", serde_json::json!({ "duration_seconds": 10.0 }), none.clone(), 0.32),
            ("minimax/MiniMax-Hailuo-2.3-Fast", serde_json::json!({ "resolution": "1080P" }), none.clone(), 0.33),
            // Vidu 2.0 (credits at $0.005): 4 s 360p 20, 720p 40, 1080p 100; 8 s 720p 100; reference 80.
            ("vidu/vidu2.0", serde_json::json!({}), none.clone(), 0.20),
            ("vidu/vidu2.0", serde_json::json!({ "resolution": "360p" }), none.clone(), 0.10),
            ("vidu/vidu2.0", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.50),
            ("vidu/vidu2.0", serde_json::json!({ "duration_seconds": 8 }), none.clone(), 0.50),
            ("vidu/vidu2.0", serde_json::json!({}), serde_json::json!([{ "type": "url", "value": "https://x/a.png", "role": "reference" }]), 0.40),
            // Vidu Q2 Pro image-to-video: 720p 15 + 10/s; 1080p 55 + 15/s; 540p 8 + 10 + 5/s.
            ("vidu/viduq2-pro", serde_json::json!({}), none.clone(), 0.275),
            ("vidu/viduq2-pro", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.575),
            ("vidu/viduq2-pro", serde_json::json!({ "resolution": "540p" }), none.clone(), 0.165),
            // Q2-Pro reference-to-video: 720p 30 + 5/s.
            ("vidu/viduq2-pro", serde_json::json!({}), serde_json::json!([{ "type": "url", "value": "https://x/a.png", "role": "reference" }]), 0.25),
            // Gemini 3 Pro Image: 1K/2K $0.134, 4K $0.24.
            ("google/gemini-3-pro-image", serde_json::json!({ "resolution": "4K" }), none.clone(), 0.24),
            ("google/gemini-3-pro-image", serde_json::json!({ "resolution": "2K" }), none.clone(), 0.134),
            // Gemini 3.1 Flash Image: "$0.045 per 0.5K image … $0.067 per 1K image,
            // $0.101 per 2K image, and $0.151 per 4K image".
            ("google/gemini-3.1-flash-image", serde_json::json!({ "resolution": "0.5K" }), none.clone(), 0.045),
            ("google/gemini-3.1-flash-image", serde_json::json!({ "resolution": "2K" }), none.clone(), 0.101),
            ("google/gemini-3.1-flash-image", serde_json::json!({ "resolution": "4K" }), none.clone(), 0.151),
            // Nano Banana 2.1: $0.0336 per 1K, $0.0504 per 2K, $0.113 per 4K.
            ("google/gemini-nano-banana-2.1", serde_json::json!({ "resolution": "2K" }), none.clone(), 0.0504),
            // Input images past the free ones (2026-11-03): Seedream 5.0 Pro "From the
            // second image: $0.003"; MiniMax H3 / Highspeed / Lite "The first 5 images
            // per task are free" ($0.04 / $0.03 / $0.02 each after), H3-Max "The first
            // 2 images" ($0.074 each after).
            ("bytedance/dola-seedream-5-0-pro-260628", serde_json::json!({}), refs(1), 0.09),
            ("bytedance/dola-seedream-5-0-pro-260628", serde_json::json!({}), refs(3), 0.096),
            ("bytedance/dola-seedream-5-0-pro-260628", serde_json::json!({}), refs(10), 0.117),
            ("minimax/MiniMax-H3", serde_json::json!({}), refs(5), 0.40),
            ("minimax/MiniMax-H3", serde_json::json!({}), refs(6), 0.44),
            ("minimax/MiniMax-H3-Highspeed", serde_json::json!({}), refs(7), 0.36),
            ("minimax/MiniMax-H3-Lite", serde_json::json!({ "resolution": "480P" }), refs(9), 0.205),
            ("minimax/MiniMax-H3-Max", serde_json::json!({}), refs(2), 0.40),
            ("minimax/MiniMax-H3-Max", serde_json::json!({}), refs(9), 0.918),
            // Pixel tiers and bands (2026-11-03). FLUX 3: a custom size "is billed at the
            // smallest `resolution` with at least as many pixels" (768², 1024², 1536²,
            // 2048², then 4k), and width/height override a tier.
            ("bfl/flux-3-image", serde_json::json!({ "size": "512x512" }), none.clone(), 0.041),
            ("bfl/flux-3-image", serde_json::json!({ "size": "1024x1024" }), none.clone(), 0.048),
            ("bfl/flux-3-image", serde_json::json!({ "size": "1536x1536" }), none.clone(), 0.07),
            ("bfl/flux-3-image", serde_json::json!({ "size": "2560x1440" }), none.clone(), 0.10),
            ("bfl/flux-3-image", serde_json::json!({ "size": "4096x4096" }), none.clone(), 0.607),
            ("bfl/flux-3-image", serde_json::json!({ "size": "1024x1024", "resolution": "4K" }), none.clone(), 0.048),
            ("bfl/flux-3-image", serde_json::json!({ "resolution": "match_input" }), none.clone(), 0.10),
            ("bfl/flux-3-image-klein", serde_json::json!({ "size": "2048x1152" }), none.clone(), 0.011),
            ("bfl/flux-3-image-klein", serde_json::json!({ "size": "3000x2000" }), none.clone(), 0.036),
            // Seedream 5.0 Pro: "Up to 2.61 million pixels (1.5K or lower): $0.045".
            ("bytedance/dola-seedream-5-0-pro-260628", serde_json::json!({ "size": "1600x1600" }), none.clone(), 0.045),
            ("bytedance/dola-seedream-5-0-pro-260628", serde_json::json!({ "size": "2048x2048" }), none.clone(), 0.09),
            ("bytedance/dola-seedream-5-0-pro-260628", serde_json::json!({ "size": "1600x1600" }), refs(3), 0.051),
            // FLUX.2: the first megapixel flat, each further one rounded up (1 MP = 1024²).
            ("bfl/flux-2-pro", serde_json::json!({ "size": "1024x1024" }), none.clone(), 0.03),
            ("bfl/flux-2-pro", serde_json::json!({ "size": "1440x1440" }), none.clone(), 0.045),
            ("bfl/flux-2-pro", serde_json::json!({ "size": "2048x2048" }), none.clone(), 0.075),
            ("bfl/flux-2-max", serde_json::json!({ "size": "2048x1024" }), none.clone(), 0.10),
            ("bfl/flux-2-klein-9b", serde_json::json!({ "size": "2048x2048" }), none.clone(), 0.021),
            ("bfl/flux-2-flex", serde_json::json!({ "size": "1440x1440" }), none.clone(), 0.10),
            ("bfl/flux-2-flex", serde_json::json!({ "size": "2048x2048" }), none.clone(), 0.20),
            // Vidu Q2: "The direct audio/video output feature for image-to-video/
            // reference-to-video consumes an additional 15 credits".
            ("vidu/viduq2-pro", serde_json::json!({ "audio": true }), none.clone(), 0.35),
            ("vidu/viduq2-pro", serde_json::json!({ "audio": false }), none.clone(), 0.275),
            // Vidu Q3 / Q3 Mix / Q3 Pro Fast (credits a second), Q2 and Q2 Turbo (the first
            // second, then a rate), Q1 Classic (80 credits).
            ("vidu/viduq3", serde_json::json!({ "resolution": "1080p", "duration_seconds": 10 }), none.clone(), 0.75),
            ("vidu/viduq3-mix", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.725),
            ("vidu/viduq3-pro-fast", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.375),
            ("vidu/viduq2", serde_json::json!({ "resolution": "540p", "duration_seconds": 10 }), none.clone(), 0.14),
            ("vidu/viduq2", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.30),
            ("vidu/viduq2-turbo", serde_json::json!({ "duration_seconds": 2 }), none.clone(), 0.09),
            ("vidu/viduq2-turbo", serde_json::json!({ "resolution": "1080p", "duration_seconds": 8 }), none.clone(), 0.525),
            // TokenHub's resold Vidu (2026-11-04): per second, and Q2's first second + rate.
            ("tokenhub/vidu-video-q4-preview", serde_json::json!({ "resolution": "4K", "duration_seconds": 10 }), refs(1), 3.653),
            ("tokenhub/vidu-video-q3-turbo", serde_json::json!({ "resolution": "540p" }), refs(0), 0.164),
            ("tokenhub/vidu-video-q2", serde_json::json!({}), none.clone(), 0.1638),
            ("tokenhub/vidu-video-q2-pro", serde_json::json!({ "resolution": "540p", "duration_seconds": 1 }), refs(1), 0.0375),
            ("tokenhub/vidu-video-q2-pro", serde_json::json!({ "resolution": "1080p", "audio": true }), refs(1), 0.6091),
            ("tokenhub/vidu-video-q2-turbo", serde_json::json!({}), refs(1), 0.2246),
            ("tokenhub/vidu-video-q2-turbo", serde_json::json!({ "resolution": "1080p", "duration_seconds": 8 }), refs(1), 0.4915),
            ("vidu/viduq2-turbo", serde_json::json!({ "resolution": "540p" }), none.clone(), 0.07),
            ("vidu/viduq2-turbo", serde_json::json!({ "audio": true }), none.clone(), 0.315),
            ("vidu/viduq1-classic", serde_json::json!({}), none.clone(), 0.40),
            // Vidu Q4 Preview: 9 / 19 / 24 / 38 / 78 credits a second ($0.005 a credit).
            ("vidu/viduq4-preview", serde_json::json!({}), none.clone(), 0.475),
            ("vidu/viduq4-preview", serde_json::json!({ "resolution": "540p", "duration_seconds": 10 }), none.clone(), 0.45),
            ("vidu/viduq4-preview", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.6),
            ("vidu/viduq4-preview", serde_json::json!({ "resolution": "2K" }), none.clone(), 0.95),
            ("vidu/viduq4-preview", serde_json::json!({ "resolution": "4K", "duration_seconds": 16 }), none.clone(), 6.24),
            // Ideogram 4.5: Generate Low / Medium / High $0.03 / $0.06 / $0.10; with
            // source images Very Low … High $0.008 / $0.03 / $0.06 / $0.22.
            ("ideogram/ideogram-4-5", serde_json::json!({}), none.clone(), 0.10),
            ("ideogram/ideogram-4-5", serde_json::json!({ "quality": "low" }), none.clone(), 0.03),
            ("ideogram/ideogram-4-5", serde_json::json!({}), refs(1), 0.06),
            ("ideogram/ideogram-4-5", serde_json::json!({ "quality": "high" }), refs(2), 0.22),
            ("ideogram/ideogram-4-5", serde_json::json!({ "quality": "very_low" }), refs(1), 0.008),
            // FLUX 3 Image on Replicate: "$0.0205 … when target resolution equals 768sq", … $0.3035 at 4k.
            ("replicate/flux-3-image", serde_json::json!({}), none.clone(), 0.024),
            ("replicate/flux-3-image", serde_json::json!({ "resolution": "4K" }), none.clone(), 0.3035),
            // FLUX 3 Image and Klein: per image, by `resolution` (1k when omitted).
            ("bfl/flux-3-image", serde_json::json!({}), none.clone(), 0.048),
            ("bfl/flux-3-image", serde_json::json!({ "resolution": "4K" }), none.clone(), 0.607),
            ("bfl/flux-3-image", serde_json::json!({ "resolution": "0.75K" }), none.clone(), 0.041),
            ("bfl/flux-3-image-klein", serde_json::json!({ "resolution": "2K" }), none.clone(), 0.014),
            ("bfl/flux-3-image-klein", serde_json::json!({ "resolution": "1.5K" }), none.clone(), 0.011),
            // Kling v2.1 image: text-to-image 4 units, image-to-image 8.
            ("kling/kling-v2-1", serde_json::json!({}), serde_json::json!([{ "type": "url", "value": "https://x/a.png" }]), 0.028),
            // Hunyuan3D 3.1 Pro: $0.375; "Using multi-view images adds $0.15",
            // and a lone front view is the plain image-to-3D input.
            ("fal/hunyuan3d-v3.1-pro", serde_json::json!({}), serde_json::json!([{ "type": "url", "value": "https://x/f.png", "role": "view-front" }]), 0.375),
            (
                "fal/hunyuan3d-v3.1-pro",
                serde_json::json!({}),
                serde_json::json!([
                    { "type": "url", "value": "https://x/f.png", "role": "view-front" },
                    { "type": "url", "value": "https://x/b.png", "role": "view-back" }
                ]),
                0.525,
            ),
            (
                "fal/hunyuan3d-v3.1-pro",
                serde_json::json!({}),
                serde_json::json!([
                    { "type": "url", "value": "https://x/f.png", "role": "init" },
                    { "type": "url", "value": "https://x/l.png", "role": "view-left" },
                    { "type": "url", "value": "https://x/r.png", "role": "view-right" }
                ]),
                0.525,
            ),
            // The 2026-11-09 audit (docs/superpowers/2026-11-09-provider-audit.md), each
            // re-read that day. GPT Image 2 at 1024x1024: "Low $0.006 · Medium $0.053 ·
            // High $0.211" (image-generation guide, "Historical output costs").
            ("openai/gpt-image-2", serde_json::json!({}), none.clone(), 0.053),
            ("openai/gpt-image-2", serde_json::json!({ "quality": "low" }), none.clone(), 0.006),
            ("openai/gpt-image-2", serde_json::json!({ "quality": "high" }), none.clone(), 0.211),
            // Ideogram 3.0 "with Character Reference": Turbo $0.10, Default $0.15, Quality
            // $0.20; a style reference keeps the model's rate.
            ("ideogram/ideogram-v3", serde_json::json!({}), refs(1), 0.06),
            ("ideogram/ideogram-v3", serde_json::json!({}), serde_json::json!([{ "type": "url", "value": "https://x/c.png", "role": "character" }]), 0.15),
            ("ideogram/ideogram-v3-turbo", serde_json::json!({}), serde_json::json!([{ "type": "url", "value": "https://x/c.png", "role": "character" }]), 0.10),
            ("ideogram/ideogram-v3-quality", serde_json::json!({}), serde_json::json!([{ "type": "url", "value": "https://x/c.png", "role": "character" }]), 0.20),
            // Gemini 2.5 Flash Image "$0.039 per image"; 3.1 Flash Lite "equivalent to
            // $0.0336 per image" at 1K, its only size.
            ("google/gemini-2.5-flash-image", serde_json::json!({}), none.clone(), 0.039),
            ("google/gemini-3.1-flash-lite-image", serde_json::json!({}), none.clone(), 0.0336),
            // Vidu credits a second at 540p / 720p / 1080p: Q3 Turbo 7 / 12 / 13, Q3 Pro
            // 9 / 20 / 24; Q2 Pro Fast 1080P "16 credits for the 1st second, +4
            // credits/second thereafter" (credits at $0.005).
            ("vidu/viduq3-turbo", serde_json::json!({}), none.clone(), 0.30),
            ("vidu/viduq3-turbo", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.325),
            ("vidu/viduq3-turbo", serde_json::json!({ "resolution": "540p", "duration_seconds": 10 }), none.clone(), 0.35),
            ("vidu/viduq3-pro", serde_json::json!({}), none.clone(), 0.50),
            ("vidu/viduq3-pro", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.60),
            ("vidu/viduq3-pro", serde_json::json!({ "resolution": "540p" }), none.clone(), 0.225),
            ("vidu/viduq2-pro-fast", serde_json::json!({}), none.clone(), 0.08),
            ("vidu/viduq2-pro-fast", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.16),
            ("vidu/viduq2-pro-fast", serde_json::json!({ "resolution": "1080p", "duration_seconds": 1 }), none.clone(), 0.08),
            // Luma "Create cost (for 5s Ray 2 video)": 1080p 0.95, 4k 1.05.
            ("luma/ray-2", serde_json::json!({}), none.clone(), 0.40),
            ("luma/ray-2", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.95),
            ("luma/ray-2", serde_json::json!({ "resolution": "4k" }), none.clone(), 1.05),
            ("luma/dream-machine", serde_json::json!({ "resolution": "1080p" }), none.clone(), 0.95),
            ("luma/dream-machine", serde_json::json!({ "resolution": "4k" }), none.clone(), 1.05),
        ];
        let wrong: Vec<String> = cases
            .into_iter()
            .filter_map(|(id, params, refs, want)| {
                let got = quote(id, params.clone(), refs);
                ((got - want).abs() > 1e-6).then(|| format!("{id} {params}: {got} (want {want})"))
            })
            .collect();
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
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

#[cfg(test)]
mod refresh_guard_tests {
    use super::*;
    use std::collections::HashSet;

    fn ids(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn model(id: &str, name: &str) -> ModelSchema {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "provider": "mock",
            "media_type": "image",
            "display_name": name,
            "pricing": { "base_cost_usd": 1.0 },
            "capabilities": { "text_to_image": true },
            "prompt": { "required": true }
        }))
        .expect("a model schema")
    }

    /// A refresh holds the disabled set to the database's, unless a change was
    /// made here after it read the version: that change stands, and the next
    /// refresh applies the database, which holds it.
    #[test]
    fn the_disabled_set_is_replaced_only_if_nothing_changed_it_since() {
        let r = CapabilityRegistry::default();
        let v = r.disabled_version();
        assert_eq!(r.replace_disabled_unless_changed(v, ids(&["a/x"])), Some(true), "applied");
        assert!(r.is_disabled("a/x"));
        let v = r.disabled_version();
        assert_eq!(r.replace_disabled_unless_changed(v, ids(&["a/x"])), Some(false), "the same set changes nothing");
        let v = r.disabled_version();
        r.set_disabled("a/y", true); // a PATCH here, after the refresh read the version
        assert_eq!(r.replace_disabled_unless_changed(v, ids(&[])), None, "refused: the PATCH stands");
        assert!(r.is_disabled("a/x") && r.is_disabled("a/y"));
        assert_eq!(r.replace_disabled_unless_changed(r.disabled_version(), ids(&[])), Some(true), "a fresh read applies");
    }

    /// Of two refreshes that read the same version, the second stands down
    /// once the first changed the set (the next tick applies the database).
    #[test]
    fn a_replace_that_changed_the_set_makes_an_overlapping_one_stand_down() {
        let r = CapabilityRegistry::default();
        let v = r.disabled_version();
        assert_eq!(r.replace_disabled_unless_changed(v, ids(&["a/x"])), Some(true));
        assert_eq!(r.replace_disabled_unless_changed(v, ids(&[])), None, "it read before the first one applied");
        assert!(r.is_disabled("a/x"));
        let v = r.disabled_version();
        assert_eq!(r.replace_disabled_unless_changed(v, ids(&["a/x"])), Some(false));
        assert_eq!(r.replace_disabled_unless_changed(v, ids(&["a/x"])), Some(false), "one that changed nothing leaves the version");
    }

    /// The same guard for the runtime layer: an update a refresh planned from
    /// its read is refused once a save here changed the layer after it read
    /// the version.
    #[test]
    fn the_runtime_layer_is_updated_only_if_nothing_changed_it_since() {
        let r = CapabilityRegistry::default();
        let v = r.runtime_version();
        assert_eq!(r.update_runtime_unless_changed(v, vec![model("m/a", "one")], &[]), Some(true));
        assert_eq!(r.runtime_model("m/a").unwrap().display_name, "one");
        let v = r.runtime_version();
        assert_eq!(r.update_runtime_unless_changed(v, vec![], &["m/none".to_string()]), Some(false), "nothing to remove");
        let v = r.runtime_version();
        r.remove_runtime("m/a"); // a DELETE here, after the refresh read the version
        assert_eq!(r.update_runtime_unless_changed(v, vec![model("m/a", "one")], &[]), None, "refused: the DELETE stands");
        assert!(r.runtime_model("m/a").is_none());
        let v = r.runtime_version();
        r.upsert_runtime(model("m/b", "two")); // a POST or PUT here
        assert_eq!(r.update_runtime_unless_changed(v, vec![], &["m/b".to_string()]), None, "refused: the save stands");
        assert_eq!(r.runtime_model("m/b").unwrap().display_name, "two");
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    fn registry(retires_at: &str) -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("t.yaml", &format!(r#"
models:
  - id: x/old
    provider: x
    media_type: image
    display_name: Old
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ text_to_image: true }}
    prompt: {{ required: true }}
    deprecated_at: 2020-01-01
    retires_at: {retires_at}
    replaced_by: x/new
  - id: x/new
    provider: x
    media_type: image
    display_name: New
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ text_to_image: true }}
    prompt: {{ required: true }}
"#))]).unwrap()
    }

    /// L14: from the day after its retirement a model is neither listed nor
    /// resolved for a request; until then it is served, as deprecated.
    #[test]
    fn a_model_past_its_retirement_is_no_longer_served() {
        let gone = registry("2020-06-30");
        assert!(gone.served().iter().all(|m| m.id != "x/old"));
        assert!(gone.resolve("x/old").is_none());
        let going = registry("2999-12-31");
        let old = going.resolve("x/old").expect("still served");
        assert_eq!(old.status(), crate::capabilities::ModelStatus::Deprecated);
        assert_eq!(going.resolve("x/new").unwrap().status(), crate::capabilities::ModelStatus::Active);
    }

    /// A deprecation dated ahead is the vendor's notice: the model is active
    /// until that day, deprecated from it. A shutdown date marks it
    /// deprecated at once, since the end is announced.
    #[test]
    fn a_deprecation_dated_ahead_starts_on_its_day() {
        let r = CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: x/later
    provider: x
    media_type: image
    display_name: Later
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    deprecated_at: 2030-01-01
"#)]).unwrap();
        let m = r.get("x/later").unwrap();
        let day = |d: &str| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
        assert_eq!(m.status_on(day("2029-12-31")), crate::capabilities::ModelStatus::Active);
        assert_eq!(m.status_on(day("2030-01-01")), crate::capabilities::ModelStatus::Deprecated);
        let going = registry("2999-12-31");
        assert_eq!(going.get("x/old").unwrap().status_on(day("2019-01-01")), crate::capabilities::ModelStatus::Deprecated, "a shutdown date is announced at once");
    }

    #[test]
    fn the_catalog_version_changes_with_any_field() {
        let a = registry("2999-12-31").catalog_version();
        assert_eq!(a, registry("2999-12-31").catalog_version());
        assert_ne!(a, registry("2999-12-30").catalog_version());
    }

    /// Two processes or replicas serving one catalog report one version: the
    /// fingerprint cannot follow a map's per-instance iteration order.
    #[test]
    fn the_catalog_version_is_a_function_of_the_catalog_alone() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../models");
        let versions: Vec<String> = (0..4).map(|_| CapabilityRegistry::from_dir(&dir).unwrap().catalog_version()).collect();
        assert!(versions.windows(2).all(|w| w[0] == w[1]), "{versions:?}");
    }
}
