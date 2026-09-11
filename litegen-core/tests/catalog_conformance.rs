//! Catalog conformance: every model id and enumerated value we advertise in
//! `models/*.yaml` must still be one the vendor accepts.
//!
//! These assertions encode facts read out of vendor documentation on
//! 2026-08-16 (see `docs/superpowers/2026-08-16-provider-api-audit.md`). When a
//! vendor moves, one of these fails and points at the row to re-verify — which
//! is the whole reason they are data-driven rather than baked into the adapter
//! unit tests.

use litegen::capabilities::schema::ParamSpec;
use litegen::capabilities::CapabilityRegistry;

fn registry() -> CapabilityRegistry {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("models");
    CapabilityRegistry::from_dir(&p).expect("load models/")
}

fn aspect_ratios(reg: &CapabilityRegistry, id: &str) -> Vec<String> {
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    match schema.params.get("aspect_ratio") {
        Some(ParamSpec::AspectRatio(ar)) => ar.allowed.clone(),
        None => Vec::new(),
        other => panic!("{id} aspect_ratio is {other:?}, expected AspectRatio"),
    }
}

fn enum_values(reg: &CapabilityRegistry, id: &str, param: &str) -> Vec<String> {
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    match schema.params.get(param) {
        Some(ParamSpec::String(s)) => s.enum_values.clone(),
        None => Vec::new(),
        other => panic!("{id} {param} is {other:?}, expected String"),
    }
}

/// Model ids whose vendor shutdown date has already passed. Advertising any of
/// these guarantees a runtime failure.
#[test]
fn no_retired_model_ids_are_advertised() {
    let reg = registry();
    // (catalog id, what happened)
    let retired: &[(&str, &str)] = &[
        ("openai/dall-e-2", "OpenAI shutdown 2026-05-12"),
        ("openai/dall-e-3", "OpenAI shutdown 2026-05-12"),
        ("google/imagen-3", "imagen-3.0-generate-002 shutdown 2025-11-10"),
        ("google/veo-2.0-generate-001", "Google shutdown 2026-06-30"),
        // Deprecated and past their earliest shutdown date, but not yet shut
        // down as of 2026-09-11 (their deprecations rows are not grayed); veo-3.1
        // is the replacement, so they stay out of the catalog.
        // @see https://ai.google.dev/gemini-api/docs/deprecations
        ("google/veo-3.0-generate-001", "deprecated; earliest shutdown 2026-06-30"),
        ("google/veo-3.0-fast-generate-001", "deprecated; earliest shutdown 2026-06-30"),
        ("runway/gen-3", "gen3a_turbo removed from the API 2026-07-30"),
        ("runway/gen-3-turbo", "gen3a_turbo removed from the API 2026-07-30"),
        ("bfl/flux-pro", "no /v1/flux-pro endpoint in api.bfl.ai/openapi.json"),
        // 2026-09-11 model-drift decisions (docs/superpowers/2026-09-11-model-drift-decisions.md):
        ("leonardo/veo3", "Leonardo retired Veo 3 (VEO3) 2026-06-29; replacement leonardo/veo3.1"),
        ("luma/ray-3", "not in the Dream Machine model enum (ray-2 | ray-flash-2); ray-3.2 is Luma Agents API only"),
        ("luma/ray-hdr-3", "not in the Dream Machine model enum (ray-2 | ray-flash-2); ray-3.2 is Luma Agents API only"),
        ("bytedance/seedream-3-0-t2i-250415", "BytePlus deactivated it 2026-05-13; replacement bytedance/seedream-5-0-lite-260128"),
        ("bytedance/doubao-seedance-1-0-lite-i2v-250428", "BytePlus deactivated it 2026-05-13; replacement bytedance/seedance-1-0-pro-fast-251015"),
    ];
    let mut still_present = Vec::new();
    for (id, why) in retired {
        if reg.get(id).is_some() {
            still_present.push(format!("  {id} — {why}"));
        }
    }
    assert!(
        still_present.is_empty(),
        "retired models are still advertised:\n{}",
        still_present.join("\n")
    );
}

/// Carried models whose vendor has announced a shutdown date: (catalog id,
/// shutdown date, what happens). They stay advertised while the vendor still
/// serves them; from the day after the date, `models_past_their_announced_shutdown_are_removed`
/// fails until the row is removed from `models/*.yaml` (and its entry from
/// `scripts/model-drift/providers/<p>.mjs`) and the id moves to
/// `no_retired_model_ids_are_advertised`. The weekly `/model-drift` run adds
/// entries here as vendors announce shutdowns.
const SCHEDULED_SHUTDOWNS: &[(&str, &str, &str)] = &[
    // @see https://kling.ai/document-api/api/image/2-0/image-generation.md
    ("kling/kling-v2", "2026-09-15", "Kling retires Image 2.0; successor kling/kling-v2-1"),
    ("kling/kling-v1-5", "2026-09-15", "Kling retires Image 1.5; successor kling/kling-v2-1"),
    ("kling/video-kling-v1-6", "2026-09-15", "Kling retires Video 1.6; successors video-kling-v2-5-turbo / -v2-6"),
    ("kling/video-kling-v2-1", "2026-09-15", "Kling retires Video 2.1; successors video-kling-v2-5-turbo / -v2-6"),
    // @see https://developers.openai.com/api/docs/deprecations
    ("openai/sora", "2026-09-24", "OpenAI shuts down the Videos API and Sora 2; no replacement"),
    ("openai/sora-2-pro", "2026-09-24", "OpenAI shuts down the Videos API and Sora 2; no replacement"),
    ("openai/gpt-image-1", "2026-10-23", "OpenAI shuts down gpt-image-1; replacement openai/gpt-image-2"),
    // @see https://docs.aws.amazon.com/bedrock/latest/userguide/model-lifecycle.html
    ("bedrock/amazon.nova-canvas-v1:0", "2026-09-30", "Bedrock end of life for Nova Canvas; no Amazon successor"),
    ("bedrock/amazon.nova-reel-v1:1", "2026-09-30", "Bedrock end of life for Nova Reel; no Amazon successor"),
    // @see https://ai.google.dev/gemini-api/docs/deprecations
    ("google/gemini-2.5-flash-image", "2026-10-02", "Google shuts down gemini-2.5-flash-image; successor google/gemini-3.1-flash-image"),
];

/// Strictly after the shutdown date: on the date itself the vendor still serves it.
fn overdue(today: chrono::NaiveDate, shutdown: &str) -> bool {
    let date = chrono::NaiveDate::parse_from_str(shutdown, "%Y-%m-%d")
        .unwrap_or_else(|e| panic!("shutdown date {shutdown:?} is not YYYY-MM-DD: {e}"));
    today > date
}

#[test]
fn overdue_starts_the_day_after_the_shutdown_date() {
    let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
    assert!(!overdue(d("2026-09-14"), "2026-09-15"));
    assert!(!overdue(d("2026-09-15"), "2026-09-15"));
    assert!(overdue(d("2026-09-16"), "2026-09-15"));
    assert!(overdue(d("2027-01-01"), "2026-12-31"));
}

#[test]
fn models_past_their_announced_shutdown_are_removed() {
    let reg = registry();
    let today = chrono::Utc::now().date_naive();
    let still_advertised: Vec<String> = SCHEDULED_SHUTDOWNS
        .iter()
        .filter(|(id, date, _)| overdue(today, date) && reg.get(id).is_some())
        .map(|(id, date, why)| format!("  {id}: shut down {date} ({why})"))
        .collect();
    assert!(
        still_advertised.is_empty(),
        "models past their vendor shutdown date are still advertised. Remove each from \
         models/*.yaml and scripts/model-drift/providers/<p>.mjs, then move it from \
         SCHEDULED_SHUTDOWNS to no_retired_model_ids_are_advertised:\n{}",
        still_advertised.join("\n")
    );
}

/// Once a scheduled model is removed, its entry must move to the retired list,
/// so SCHEDULED_SHUTDOWNS never silently guards nothing.
#[test]
fn scheduled_shutdowns_name_models_still_in_the_catalog() {
    let reg = registry();
    let gone: Vec<&str> = SCHEDULED_SHUTDOWNS
        .iter()
        .filter(|(id, _, _)| reg.get(id).is_none())
        .map(|(id, _, _)| *id)
        .collect();
    assert!(
        gone.is_empty(),
        "these SCHEDULED_SHUTDOWNS entries are no longer in the catalog; move them to \
         no_retired_model_ids_are_advertised: {gone:?}"
    );
}

/// Replacements for the retired ids must actually be present, or the fix
/// silently reduced the catalog instead of migrating it.
#[test]
fn replacement_models_are_advertised() {
    let reg = registry();
    for id in [
        "openai/gpt-image-2",
        "openai/gpt-image-1",
        "google/gemini-3.1-flash-image",
        "google/gemini-3-pro-image",
        "runway/gen4-turbo",
        "runway/gen4.5",
        // FLUX.1 [pro] has no endpoint; FLUX.2 [pro] is BFL's documented default.
        "bfl/flux-2-pro",
    ] {
        assert!(reg.get(id).is_some(), "{id} should be in the catalog");
    }
}

/// Runway's `/v1/text_to_image` `ratio` enum has no true 3:2 pair, so `3:2`
/// and `2:3` must not be advertised. (That the adapter only ever *emits* enum
/// members is covered by `resolve_ratio_only_ever_returns_enum_members`, which
/// calls the real function rather than duplicating its table here.)
#[test]
fn runway_image_does_not_advertise_unmappable_ratios() {
    let reg = registry();
    for id in ["runway/gen4_image", "runway/gen4_image_turbo"] {
        let allowed = aspect_ratios(&reg, id);
        assert!(!allowed.is_empty(), "{id} declares no aspect ratios");
        for ar in ["3:2", "2:3"] {
            assert!(
                !allowed.contains(&ar.to_string()),
                "{id} advertises {ar:?}, which Runway's ratio enum cannot express"
            );
        }
    }
}

/// `/v1/image_to_video` + `/v1/text_to_video` `ratio` enum for gen4 models.
#[test]
fn runway_video_aspect_ratios_are_in_the_gen4_enum() {
    const GEN4_RATIOS: [&str; 6] = [
        "1280:720", "720:1280", "1104:832", "832:1104", "960:960", "1584:672",
    ];
    let reg = registry();
    for id in ["runway/gen4-turbo", "runway/gen4.5"] {
        let allowed = aspect_ratios(&reg, id);
        assert!(!allowed.is_empty(), "{id} declares no aspect ratios");
        for ar in allowed {
            assert!(
                GEN4_RATIOS.contains(&ar.as_str()),
                "{id} allows {ar:?}, not in the gen4 ratio enum"
            );
        }
    }
}

/// Stability v2 `aspect_ratio` enum, shared by core/sd3/ultra.
#[test]
fn stability_aspect_ratios_are_in_the_v2_enum() {
    const V2_RATIOS: [&str; 9] = [
        "21:9", "16:9", "3:2", "5:4", "1:1", "4:5", "2:3", "9:16", "9:21",
    ];
    let reg = registry();
    for id in ["stability/sd3-large", "stability/sd3-turbo", "stability/core", "stability/ultra"] {
        for ar in aspect_ratios(&reg, id) {
            assert!(
                V2_RATIOS.contains(&ar.as_str()),
                "{id} allows {ar:?}, not in the Stability v2 aspect_ratio enum"
            );
        }
    }
}

/// `CreateImageRequest.quality`: `low|medium|high|auto` are the GPT image
/// values; `standard`/`hd` are dall-e-3-only and those models are gone.
#[test]
fn openai_image_quality_values_are_the_gpt_image_enum() {
    let reg = registry();
    for id in ["openai/gpt-image-2", "openai/gpt-image-1"] {
        for q in enum_values(&reg, id, "quality") {
            assert!(
                ["auto", "low", "medium", "high"].contains(&q.as_str()),
                "{id} allows quality {q:?}; only auto/low/medium/high apply to GPT image models"
            );
        }
    }
}

/// MiniMax `image-01` `aspect_ratio` enum, verified against the embedded
/// OpenAPI at platform.minimax.io. This one already matched — the test locks it.
#[test]
fn minimax_image_aspect_ratios_match_the_documented_enum() {
    const MINIMAX_RATIOS: [&str; 8] =
        ["1:1", "16:9", "4:3", "3:2", "2:3", "3:4", "9:16", "21:9"];
    let reg = registry();
    for ar in aspect_ratios(&reg, "minimax/image-01") {
        assert!(
            MINIMAX_RATIOS.contains(&ar.as_str()),
            "minimax/image-01 allows {ar:?}, not in the documented enum"
        );
    }
}

/// Ideogram v3 `style_type` enum.
#[test]
fn ideogram_style_values_match_the_documented_enum() {
    const STYLE_TYPES: [&str; 5] = ["AUTO", "GENERAL", "REALISTIC", "DESIGN", "FICTION"];
    let reg = registry();
    for id in ["ideogram/ideogram-v3", "ideogram/ideogram-v3-turbo", "ideogram/ideogram-v3-quality"] {
        for s in enum_values(&reg, id, "style") {
            assert!(
                STYLE_TYPES.contains(&s.as_str()),
                "{id} allows style {s:?}, not in Ideogram's style_type enum"
            );
        }
    }
}

/// Vidu's Model Map marks Text-to-Video as unsupported for `vidu2.0` and
/// `viduq2-pro`; only `viduq1` in our catalog supports it. Advertising
/// `text_to_video` routes a prompt-only request to `/ent/v2/text2video` with a
/// model that cannot serve it.
/// @see <https://platform.vidu.com/docs/model-map.md>
#[test]
fn vidu_models_only_advertise_text_to_video_when_supported() {
    let reg = registry();
    for id in ["vidu/vidu2.0", "vidu/viduq2-pro"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            !schema.capabilities.text_to_video,
            "{id} advertises text_to_video, which Vidu's Model Map says it does not support"
        );
    }
    let q1 = reg.get("vidu/viduq1").expect("viduq1 missing");
    assert!(q1.capabilities.text_to_video, "viduq1 does support text-to-video");
}

/// Recraft's seed parameter is `random_seed` and applies to all models. We
/// declared no seed at all, so a caller could not ask for a reproducible image.
#[test]
fn recraft_models_declare_a_seed_param() {
    let reg = registry();
    for id in [
        "recraft/recraftv3",
        "recraft/recraftv3_vector",
        "recraft/recraftv2",
        "recraft/recraftv4_1",
        "recraft/recraftv4_1_pro",
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            matches!(schema.params.get("seed"), Some(ParamSpec::Seed(_))),
            "{id} declares no seed param"
        );
    }
}

/// Vidu's Model Map: viduq1 is 5s only at 1080p. A 4s request is rejected.
#[test]
fn vidu_q1_duration_and_resolution_match_the_model_map() {
    let reg = registry();
    let schema = reg.get("vidu/viduq1").expect("viduq1 missing");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(5.0), "viduq1 is 5s only");
            assert_eq!(f.max, Some(5.0), "viduq1 is 5s only");
        }
        other => panic!("viduq1 duration_seconds is {other:?}"),
    }
    match schema.params.get("resolution") {
        Some(ParamSpec::String(s)) => {
            assert_eq!(s.enum_values, vec!["1080p".to_string()], "viduq1 is 1080p only");
        }
        other => panic!("viduq1 resolution is {other:?}"),
    }
}

/// Nova Reel's `TEXT_VIDEO` task supports `durationSeconds: 6` only; longer
/// clips need `MULTI_SHOT_AUTOMATED`, which the adapter never emits.
/// @see <https://docs.aws.amazon.com/nova/latest/userguide/video-gen-access.html>
#[test]
fn bedrock_nova_reel_duration_matches_the_text_video_task() {
    let reg = registry();
    let schema = reg
        .get("bedrock/amazon.nova-reel-v1:1")
        .expect("nova-reel missing");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.max, Some(6.0), "TEXT_VIDEO supports 6s only");
        }
        other => panic!("nova-reel duration_seconds is {other:?}"),
    }
}

// ─── 3D family conformance ──────────────────────────────────────────────────
//
// No vendor 3D adapters ship yet, so these assert the family's structural
// contract across every advertised model3d row rather than vendor-specific ids.
// They gain subjects, not new code, when Meshy/Tripo3D land.

use litegen::capabilities::MediaType;

fn model3d_models(reg: &CapabilityRegistry) -> Vec<&litegen::capabilities::ModelSchema> {
    reg.all().filter(|m| m.media_type == MediaType::Model3d).collect()
}

#[test]
fn every_3d_model_declares_at_least_one_generation_mode() {
    let reg = registry();
    let models = model3d_models(&reg);
    assert!(!models.is_empty(), "the catalog advertises no model3d models");
    for m in models {
        let c = &m.capabilities;
        assert!(
            c.text_to_3d || c.image_to_3d || c.multiview_to_3d,
            "{} is media_type model3d but advertises no 3D generation mode — \
             a client filtering on capabilities would never offer it",
            m.id
        );
    }
}

#[test]
fn every_3d_model_advertising_image_modes_declares_the_matching_ref_roles() {
    // Mode is INFERRED from reference roles, never passed explicitly, so a model
    // claiming image_to_3d with no `init` role can never actually be driven in
    // that mode.
    let reg = registry();
    for m in model3d_models(&reg) {
        if m.capabilities.image_to_3d {
            let roles = m.ref_inputs.as_ref().map(|r| &r.roles);
            assert!(
                roles.is_some_and(|r| r.contains_key("init")),
                "{} advertises image_to_3d but declares no `init` ref role",
                m.id
            );
        }
        if m.capabilities.multiview_to_3d {
            let roles = m.ref_inputs.as_ref().expect("multiview model needs ref_inputs");
            let views = roles.roles.keys().filter(|k| k.starts_with("view-")).count();
            assert!(
                views >= 2,
                "{} advertises multiview_to_3d but declares {} view-* roles; \
                 multiview needs at least two",
                m.id, views
            );
        }
    }
}

#[test]
fn model3d_enum_vocabularies_match_the_documented_vendor_sets() {
    // Values transcribed from the design spec §9 vendor tables. If a future
    // catalog row advertises something outside these, either the vendor moved
    // or the row is wrong — either way, re-verify before widening this list.
    let reg = registry();
    for m in model3d_models(&reg) {
        if let Some(ParamSpec::String(s)) = m.params.get("topology") {
            for v in &s.enum_values {
                assert!(
                    matches!(v.as_str(), "triangle" | "quad"),
                    "{}: topology '{}' is outside the documented set {{triangle, quad}}",
                    m.id, v
                );
            }
        }
        if let Some(ParamSpec::String(s)) = m.params.get("symmetry") {
            for v in &s.enum_values {
                assert!(
                    matches!(v.as_str(), "off" | "auto" | "on"),
                    "{}: symmetry '{}' is outside the documented set {{off, auto, on}}",
                    m.id, v
                );
            }
        }
        if let Some(ParamSpec::String(s)) = m.params.get("output_format") {
            assert!(!s.enum_values.is_empty(), "{}: output_format declares no values", m.id);
            for v in &s.enum_values {
                assert!(
                    matches!(v.as_str(), "glb" | "obj" | "fbx" | "usdz" | "stl" | "3mf"),
                    "{}: output_format '{}' is not a mesh container litegen supports",
                    m.id, v
                );
            }
            assert!(
                s.enum_values.iter().any(|v| v == "glb"),
                "{}: every 3D model must be able to emit glb — it is the only \
                 self-contained format the dashboard viewer and the SDK assume",
                m.id
            );
        }
    }
}

#[test]
fn no_3d_model_advertises_image_or_video_only_params() {
    // A 3D row carrying `size`/`fps`/`duration_seconds` would render nonsense
    // controls in the Playground and in any schema-driven client panel.
    let reg = registry();
    for m in model3d_models(&reg) {
        for forbidden in ["size", "fps", "duration_seconds", "aspect_ratio"] {
            assert!(
                !m.params.contains_key(forbidden),
                "{} is a 3D model but advertises the {} param",
                m.id, forbidden
            );
        }
    }
}

#[test]
fn target_polycount_bounds_are_sane_where_declared() {
    let reg = registry();
    for m in model3d_models(&reg) {
        if let Some(ParamSpec::Int(i)) = m.params.get("target_polycount") {
            let (min, max) = (i.min.unwrap_or(0), i.max.unwrap_or(i64::MAX));
            assert!(min > 0, "{}: target_polycount min must be positive, got {}", m.id, min);
            assert!(max > min, "{}: target_polycount max {} <= min {}", m.id, max, min);
            assert!(
                max <= 10_000_000,
                "{}: target_polycount max {} is implausible; no phase-1 vendor \
                 accepts anything near it (Meshy tops out at 300k)",
                m.id, max
            );
        }
    }
}

// ─── Runway: vendor facts re-read 2026-09-11 (model-drift) ──────────────────

/// `gen4_turbo` appears only in the `POST /v1/image_to_video` model union
/// (`promptImage` required); `/v1/text_to_video` has no gen4_turbo branch, and
/// the model guide lists its input as "Image". Advertising text-to-video sent
/// prompt-only requests to an endpoint that rejects the model.
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-09-11)
/// @see <https://docs.dev.runwayml.com/guides/models.md> (2026-09-11)
#[test]
fn runway_gen4_turbo_is_image_to_video_only() {
    let reg = registry();
    let schema = reg.get("runway/gen4-turbo").expect("runway/gen4-turbo missing");
    assert!(
        !schema.capabilities.text_to_video,
        "gen4_turbo is not accepted by /v1/text_to_video"
    );
    assert!(schema.capabilities.image_to_video);
    let init = schema
        .ref_inputs
        .as_ref()
        .and_then(|ri| ri.roles.get("init"))
        .expect("gen4-turbo needs an init role for its required promptImage");
    assert!(
        init.required && init.min_count >= 1,
        "promptImage is required, so the init role must be required: {init:?}"
    );
}

/// `POST /v1/text_to_video` takes `ratio` ∈ `1280:720 | 720:1280` for gen4.5.
/// The catalog has one `allowed` list per model, not per mode, so a model that
/// advertises text-to-video may only offer ratios text-to-video accepts.
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-09-11)
#[test]
fn runway_gen4_5_ratios_are_in_the_text_to_video_enum() {
    const T2V_RATIOS: [&str; 2] = ["1280:720", "720:1280"];
    let reg = registry();
    let schema = reg.get("runway/gen4.5").expect("runway/gen4.5 missing");
    assert!(schema.capabilities.text_to_video, "gen4.5 is Runway's text-to-video model");
    let allowed = aspect_ratios(&reg, "runway/gen4.5");
    assert!(!allowed.is_empty(), "runway/gen4.5 declares no aspect ratios");
    for ar in allowed {
        assert!(
            T2V_RATIOS.contains(&ar.as_str()),
            "runway/gen4.5 allows {ar:?}, which /v1/text_to_video rejects for gen4.5"
        );
    }
}

/// `POST /v1/text_to_image` `promptText`: "A non-empty string up to 1000
/// characters" (`maxLength: 1000`) for gen4_image and gen4_image_turbo. The
/// catalog said 5500.
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-09-11)
#[test]
fn runway_image_prompt_limit_is_the_prompt_text_max_length() {
    let reg = registry();
    for id in ["runway/gen4_image", "runway/gen4_image_turbo"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(
            schema.prompt.max_length,
            Some(1000),
            "{id}: Runway's promptText maxLength is 1000"
        );
    }
}

/// gen4_image_turbo's `/v1/text_to_image` branch lists `referenceImages` in
/// `required` ("An array of one to three images", `minItems: 1`). A role with
/// `required: false` lets a zero-reference request through to a 400, and
/// `text_to_image` promises a prompt-only request can work.
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-09-11)
/// @see <https://docs.dev.runwayml.com/guides/models.md> (2026-09-11) — "Text+Image (References)"
#[test]
fn runway_gen4_image_turbo_requires_a_reference_image() {
    let reg = registry();
    let schema = reg
        .get("runway/gen4_image_turbo")
        .expect("runway/gen4_image_turbo missing");
    let init = schema
        .ref_inputs
        .as_ref()
        .and_then(|ri| ri.roles.get("init"))
        .expect("gen4_image_turbo needs an init role for referenceImages");
    assert!(
        init.required && init.min_count >= 1 && init.max_count == 3,
        "referenceImages is required, one to three images: {init:?}"
    );
    assert!(
        !schema.capabilities.text_to_image,
        "gen4_image_turbo cannot generate from a prompt alone"
    );
    assert!(schema.capabilities.image_to_image);
}

/// gen4.5 takes `proresProfile` on both `/v1/text_to_video` and
/// `/v1/image_to_video` ("Only valid when `outputFormat` is `prores` or
/// `hdr_prores`"). `outputFormat` is allowlisted; its companion was not, so a
/// caller could pick ProRes but not the profile.
/// @see <https://docs.dev.runwayml.com/guides/models.md#professional-and-hdr-output-formats> (2026-09-11)
#[test]
fn runway_gen4_5_allowlists_the_prores_profile_with_output_format() {
    let reg = registry();
    let schema = reg.get("runway/gen4.5").expect("runway/gen4.5 missing");
    for key in ["outputFormat", "proresProfile"] {
        assert!(
            schema.extra_allowlist.iter().any(|k| k == key),
            "runway/gen4.5 extra_allowlist is missing {key:?}: {:?}",
            schema.extra_allowlist
        );
    }
}

// ─── OpenAI: vendor facts re-read 2026-09-11 (model-drift) ──────────────────

/// The gpt-image-2 model page lists `v1/images/edits` as "Supported" and
/// inpainting as a supported feature, and `CreateImageEditRequest.model`
/// names gpt-image-2. The catalog declared text-to-image only, so a reference
/// image or mask was refused, and on 2026-10-23 gpt-image-1 (our only other
/// edit-capable OpenAI model) shuts down.
/// @see <https://developers.openai.com/api/docs/models/gpt-image-2> (2026-09-11)
#[test]
fn openai_gpt_image_2_advertises_editing_and_inpainting() {
    let reg = registry();
    let schema = reg.get("openai/gpt-image-2").expect("openai/gpt-image-2 missing");
    assert!(schema.capabilities.text_to_image);
    assert!(schema.capabilities.image_to_image, "images/edits supports gpt-image-2");
    assert!(schema.capabilities.inpainting, "gpt-image-2 lists inpainting");
    let ri = schema
        .ref_inputs
        .as_ref()
        .expect("gpt-image-2 needs ref_inputs to accept an image or mask");
    for role in ["init", "mask"] {
        assert!(ri.roles.contains_key(role), "gpt-image-2 declares no {role:?} role");
    }
    match &ri.provider_format {
        litegen::capabilities::schema::RefProviderFormat::Multipart(mp) => {
            assert_eq!(mp.field_map.get("init").map(String::as_str), Some("image"));
            assert_eq!(mp.field_map.get("mask").map(String::as_str), Some("mask"));
        }
        other => panic!("images/edits is multipart; gpt-image-2 provider_format is {other:?}"),
    }
}

/// Carried models whose vendor shutdown is announced but not yet past carry a
/// `DEPRECATED:` description naming the date, as the Sora rows already do.
/// @see <https://developers.openai.com/api/docs/deprecations> (2026-09-11)
#[test]
fn openai_models_with_an_announced_shutdown_are_marked_deprecated() {
    let reg = registry();
    // (catalog id, announced shutdown date)
    for (id, date) in [
        ("openai/gpt-image-1", "2026-10-23"),
        ("openai/sora", "2026-09-24"),
        ("openai/sora-2-pro", "2026-09-24"),
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            schema.description.starts_with("DEPRECATED") && schema.description.contains(date),
            "{id} shuts down {date}; its description should say so: {:?}",
            schema.description
        );
    }
}

// ─── MiniMax: vendor facts re-read 2026-09-11 (model-drift) ─────────────────

/// Hailuo resolutions, per model and mode (t2v / i2v / fl2v pages):
/// MiniMax-Hailuo-02 text-to-video takes `768P | 1080P`, first & last frame
/// takes `768P | 1080P` ("does not support 512P"), and only image-to-video adds
/// `512P`; MiniMax-Hailuo-2.3 takes `768P | 1080P` everywhere. Both models
/// advertise text-to-video and one list serves every mode, so the list must
/// be the text-to-video one.
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-t2v.md> (2026-09-11)
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-fl2v.md> (2026-09-11)
#[test]
fn minimax_hailuo_resolutions_are_valid_for_every_advertised_mode() {
    let reg = registry();
    for id in ["minimax/MiniMax-Hailuo-02", "minimax/MiniMax-Hailuo-2.3"] {
        let values = enum_values(&reg, id, "resolution");
        assert!(!values.is_empty(), "{id} declares no resolutions");
        for r in values {
            assert!(
                ["768P", "1080P"].contains(&r.as_str()),
                "{id} allows resolution {r:?}; text-to-video and first/last-frame reject it"
            );
        }
    }
}

/// On the **v1** API, first & last frame generation accepts only
/// `MiniMax-Hailuo-02`. A `last_frame` role on any other v1 model sends
/// `last_frame_image` with a model the fl2v request does not accept (and makes
/// /v1/models report `supports_last_frame: true`). The H3 models are exempt:
/// they go through the v2 surface, which takes roled image items.
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-fl2v.md> (2026-09-11)
#[test]
fn minimax_v1_models_other_than_hailuo_02_declare_no_last_frame() {
    let reg = registry();
    let has_last_frame = |id: &str| {
        reg.get(id)
            .unwrap_or_else(|| panic!("{id} missing"))
            .ref_inputs
            .as_ref()
            .is_some_and(|ri| ri.roles.contains_key("last_frame"))
    };
    assert!(
        has_last_frame("minimax/MiniMax-Hailuo-02"),
        "MiniMax-Hailuo-02 is the fl2v model"
    );
    for id in ["minimax/MiniMax-Hailuo-2.3", "minimax/T2V-01-Director", "minimax/S2V-01"] {
        assert!(
            !has_last_frame(id),
            "{id} declares a last_frame role, but fl2v only accepts MiniMax-Hailuo-02"
        );
    }
}

/// The S2V-01 request schema lists `subject_reference` in `required` (one
/// image: "only one image supported"). A prompt-only request cannot succeed, so
/// the role is required and the model is not text-to-video.
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-s2v.md> (2026-09-11)
#[test]
fn minimax_s2v_01_requires_its_subject_reference() {
    let reg = registry();
    let schema = reg.get("minimax/S2V-01").expect("minimax/S2V-01 missing");
    let subject = schema
        .ref_inputs
        .as_ref()
        .and_then(|ri| ri.roles.get("subject"))
        .expect("S2V-01 needs a subject role");
    assert!(
        subject.required && subject.min_count == 1 && subject.max_count == 1,
        "subject_reference is required, exactly one image: {subject:?}"
    );
    assert!(
        !schema.capabilities.text_to_video,
        "S2V-01 cannot generate from a prompt alone"
    );
}

/// `fast_pretreatment` "Applies only to `MiniMax-Hailuo-2.3` and
/// `MiniMax-Hailuo-02`" (t2v; the i2v page says the same, plus 2.3-Fast).
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-t2v.md> (2026-09-11)
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-i2v.md> (2026-09-11)
#[test]
fn minimax_hailuo_models_allowlist_fast_pretreatment() {
    let reg = registry();
    for id in ["minimax/MiniMax-Hailuo-02", "minimax/MiniMax-Hailuo-2.3"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            schema.extra_allowlist.iter().any(|k| k == "fast_pretreatment"),
            "{id} extra_allowlist is missing fast_pretreatment: {:?}",
            schema.extra_allowlist
        );
    }
}

// ─── Google: vendor facts re-read 2026-09-11 (model-drift) ──────────────────

/// Gemini 3.1 Flash Image and 3.1 Flash Lite Image each document a 14-row
/// aspect-ratio table: the 10 classic ratios (including 4:5 and 5:4, which the
/// catalog was missing) plus 1:4, 4:1, 1:8 and 8:1. The v1beta discovery doc's
/// `ImageConfig.aspectRatio` lists the same 14.
/// @see <https://ai.google.dev/gemini-api/docs/generate-content/image-generation#aspect_ratios_and_image_size> (2026-09-11)
#[test]
fn gemini_3_1_flash_image_models_advertise_the_fourteen_documented_aspect_ratios() {
    const DOCUMENTED: [&str; 14] = [
        "1:1", "1:4", "1:8", "2:3", "3:2", "3:4", "4:1", "4:3", "4:5", "5:4", "8:1", "9:16",
        "16:9", "21:9",
    ];
    let mut documented: Vec<String> = DOCUMENTED.iter().map(|s| s.to_string()).collect();
    documented.sort();
    let reg = registry();
    for id in [
        "google/gemini-3.1-flash-image",
        "google/gemini-3.1-flash-lite-image",
    ] {
        let mut allowed = aspect_ratios(&reg, id);
        allowed.sort();
        assert_eq!(
            allowed, documented,
            "{id} aspect ratios differ from Google's per-model table"
        );
    }
}

/// Gemini 3 Pro Image documents 10 aspect ratios (the table headed
/// "3.1 Pro Image", whose 1K/2K/4K sizes and 1120/2000 tokens are
/// gemini-3-pro-image's): 4:5 and 5:4 yes, the 1:4/4:1/1:8/8:1 extremes no.
/// @see <https://ai.google.dev/gemini-api/docs/generate-content/image-generation#aspect_ratios_and_image_size> (2026-09-11)
#[test]
fn gemini_3_pro_image_advertises_the_ten_documented_aspect_ratios() {
    const DOCUMENTED: [&str; 10] = [
        "1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9",
    ];
    let mut documented: Vec<String> = DOCUMENTED.iter().map(|s| s.to_string()).collect();
    documented.sort();
    let reg = registry();
    let mut allowed = aspect_ratios(&reg, "google/gemini-3-pro-image");
    allowed.sort();
    assert_eq!(
        allowed, documented,
        "google/gemini-3-pro-image aspect ratios differ from Google's table"
    );
}

/// "Gemini 3 image models let you to mix up to 14 reference images": 14 objects
/// on 3.1 Flash Lite, 10 objects + 4 characters on 3.1 Flash, 6 objects +
/// 5 characters + 3 style references on 3 Pro. The catalog capped them at 3.
/// @see <https://ai.google.dev/gemini-api/docs/generate-content/image-generation#use-14-images> (2026-09-11)
#[test]
fn gemini_3_image_models_accept_fourteen_reference_images() {
    let reg = registry();
    for id in [
        "google/gemini-3.1-flash-image",
        "google/gemini-3.1-flash-lite-image",
        "google/gemini-3-pro-image",
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        let refs = schema
            .ref_inputs
            .as_ref()
            .unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
        assert_eq!(refs.max_total, 14, "{id} ref_inputs.max_total");
        let init = refs
            .roles
            .get("init")
            .unwrap_or_else(|| panic!("{id} declares no init role"));
        assert_eq!(init.max_count, 14, "{id} init.max_count");
    }
}

/// The Gemini Developer API's `ImageConfig` has only `aspectRatio` and
/// `imageSize`; python-genai's `_ImageConfig_to_mldev` raises for
/// `person_generation` ("only supported in Gemini Enterprise Agent Platform
/// mode, not in Gemini Developer API mode"). The adapter forwards allowlisted
/// extras into `generationConfig.imageConfig`, so allowlisting it guarantees an
/// unknown-field 400 whenever it is used.
/// @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> (schemas.ImageConfig, revision 20260910)
/// @see <https://github.com/googleapis/python-genai/blob/main/google/genai/models.py> (`_ImageConfig_to_mldev`)
#[test]
fn gemini_image_models_do_not_allowlist_the_vertex_only_person_generation() {
    let reg = registry();
    for id in [
        "google/gemini-3.1-flash-image",
        "google/gemini-3.1-flash-lite-image",
        "google/gemini-3-pro-image",
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == "personGeneration"),
            "{id} allowlists personGeneration, which the Gemini Developer API's ImageConfig does not accept"
        );
    }
}

// ─── Hunyuan: vendor facts re-read 2026-09-11 (model-drift) ─────────────────

/// `SubmitHunyuanImageJob.Resolution` supports exactly eight `W:H` values
/// (default 1024:1024). The catalog's freeform 512–1280 box let through sizes
/// such as 800x900 that Tencent rejects.
/// @see <https://cloud.tencent.com/document/product/1729/105969> (2026-09-11)
#[test]
fn hunyuan_image_sizes_are_the_documented_resolution_enum() {
    let reg = registry();
    let schema = reg
        .get("hunyuan/hunyuan-image")
        .expect("hunyuan-image missing");
    let mut sizes = match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Enum(e))) => e.values.clone(),
        other => panic!("hunyuan-image size is {other:?}, expected the documented enum"),
    };
    sizes.sort();
    let mut documented: Vec<(u32, u32)> = vec![
        (768, 768),
        (768, 1024),
        (1024, 768),
        (1024, 1024),
        (720, 1280),
        (1280, 720),
        (768, 1280),
        (1280, 768),
    ];
    documented.sort();
    assert_eq!(
        sizes, documented,
        "hunyuan-image sizes differ from SubmitHunyuanImageJob.Resolution"
    );
}

/// The adapter merges every allowlisted extra key into the request verbatim,
/// and Tencent Cloud API 3.0 fails a request that carries an undefined
/// parameter (`UnknownParameter`). `RspImgType` belongs to the synchronous
/// `TextToImageLite` action, not `SubmitHunyuanImageJob`.
/// @see <https://cloud.tencent.com/document/product/1729/105969> (2026-09-11)
/// @see <https://cloud.tencent.com/document/api/1729/101847> — UnknownParameter
#[test]
fn hunyuan_image_extra_allowlist_names_only_submit_hunyuan_image_job_parameters() {
    const PARAMS: [&str; 11] = [
        "Prompt",
        "NegativePrompt",
        "Style",
        "Resolution",
        "Num",
        "Clarity",
        "ContentImage",
        "Revise",
        "Seed",
        "LogoAdd",
        "LogoParam",
    ];
    let reg = registry();
    let schema = reg
        .get("hunyuan/hunyuan-image")
        .expect("hunyuan-image missing");
    for key in &schema.extra_allowlist {
        assert!(
            PARAMS.contains(&key.as_str()),
            "hunyuan-image allowlists {key:?}, which is not a SubmitHunyuanImageJob parameter"
        );
    }
}

// ─── Ideogram: vendor facts re-read 2026-09-11 (model-drift) ────────────────

/// Ideogram 3.0's `aspect_ratio` enum (`AspectRatioV3`). We advertise `W:H`
/// and the adapter swaps ':' for 'x'; our list omitted 1x2 and 2x1.
/// @see <https://developer.ideogram.ai/openapi.json> (components.schemas.AspectRatioV3), read 2026-09-11
#[test]
fn ideogram_aspect_ratios_are_exactly_the_v3_enum() {
    const V3_RATIOS: [&str; 15] = [
        "1x3", "3x1", "1x2", "2x1", "9x16", "16x9", "10x16", "16x10", "2x3", "3x2", "3x4",
        "4x3", "4x5", "5x4", "1x1",
    ];
    let mut theirs: Vec<String> = V3_RATIOS.iter().map(|s| s.to_string()).collect();
    theirs.sort();
    let reg = registry();
    for id in ["ideogram/ideogram-v3", "ideogram/ideogram-v3-turbo", "ideogram/ideogram-v3-quality"] {
        let mut ours: Vec<String> = aspect_ratios(&reg, id).iter().map(|ar| ar.replace(':', "x")).collect();
        ours.sort();
        assert_eq!(ours, theirs, "{id} aspect ratios differ from Ideogram's AspectRatioV3 enum");
    }
}

// ─── Recraft: vendor facts re-read 2026-09-11 (model-drift) ─────────────────

/// The enumerated `size` values a Recraft model advertises.
fn recraft_sizes(reg: &CapabilityRegistry, id: &str) -> Vec<(u32, u32)> {
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Enum(e))) => {
            let mut v = e.values.clone();
            v.sort();
            v
        }
        None => Vec::new(),
        other => panic!("{id} size is {other:?}, expected an enumerated SizeSpec"),
    }
}

/// Recraft V4 / V4.1 raster sizes. We had copied the V2/V3 list, of which only
/// 1024x1024 is valid on V4.1.
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("List of supported image sizes"), read 2026-09-11
#[test]
fn recraft_v4_1_sizes_are_the_v4_table() {
    let mut v4: Vec<(u32, u32)> = vec![
        (1024, 1024), (1536, 768), (768, 1536), (1280, 832), (832, 1280), (1216, 896), (896, 1216),
        (1152, 896), (896, 1152), (832, 1344), (1280, 896), (896, 1280), (1344, 768), (768, 1344),
    ];
    v4.sort();
    assert_eq!(recraft_sizes(&registry(), "recraft/recraftv4_1"), v4);
}

/// Recraft V4 Pro / V4.1 Pro (2K) raster sizes — none of the V2/V3 sizes we
/// advertised is on this list.
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("List of supported image sizes"), read 2026-09-11
#[test]
fn recraft_v4_1_pro_sizes_are_the_v4_pro_table() {
    let mut pro: Vec<(u32, u32)> = vec![
        (2048, 2048), (3072, 1536), (1536, 3072), (2560, 1664), (1664, 2560), (2432, 1792),
        (1792, 2432), (2304, 1792), (1792, 2304), (1664, 2688), (2560, 1792), (1792, 2560),
        (2688, 1536), (1536, 2688),
    ];
    pro.sort();
    assert_eq!(recraft_sizes(&registry(), "recraft/recraftv4_1_pro"), pro);
}

/// Recraft V2 / V3 raster sizes: the Appendix gives 16:9 as 1820x1024 and 9:16
/// as 1024x1820, which we did not advertise; everything we do advertise must be
/// in the spec's V2/V3 block (which also keeps the legacy 1707x1024).
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md>
/// @see <https://external.api.recraft.ai/doc/spec/external-api.yaml> (ImageSize), read 2026-09-11
#[test]
fn recraft_v2_v3_sizes_are_the_v2_v3_table() {
    const V2_V3_SIZES: [(u32, u32); 15] = [
        (1024, 1024), (1365, 1024), (1024, 1365), (1536, 1024), (1024, 1536), (1820, 1024),
        (1024, 1820), (1024, 2048), (2048, 1024), (1434, 1024), (1024, 1434), (1024, 1280),
        (1280, 1024), (1024, 1707), (1707, 1024),
    ];
    let reg = registry();
    for id in ["recraft/recraftv3", "recraft/recraftv2"] {
        let ours = recraft_sizes(&reg, id);
        for s in &ours {
            assert!(V2_V3_SIZES.contains(s), "{id} advertises {s:?}, not a V2/V3 size");
        }
        for s in [(1820, 1024), (1024, 1820)] {
            assert!(ours.contains(&s), "{id} does not advertise {s:?} (16:9 / 9:16)");
        }
    }
}

/// Maximum prompt length: 10000 for every V4 / V4.1 model, 1000 for V2 / V3.
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("Maximum prompt length"), read 2026-09-11
#[test]
fn recraft_prompt_limits_match_the_appendix() {
    let reg = registry();
    for (id, limit) in [
        ("recraft/recraftv4_1", 10000),
        ("recraft/recraftv4_1_pro", 10000),
        ("recraft/recraftv3", 1000),
        ("recraft/recraftv3_vector", 1000),
        ("recraft/recraftv2", 1000),
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(schema.prompt.max_length, Some(limit), "{id} prompt max_length");
    }
}

/// `negative_prompt` is a V2 / V3 parameter: declared on V2 (which lacked it),
/// not on V4.1 (which had it).
/// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> (Generate image → Parameters), read 2026-09-11
#[test]
fn recraft_negative_prompt_is_declared_exactly_on_v2_v3() {
    let reg = registry();
    for id in ["recraft/recraftv3", "recraft/recraftv3_vector", "recraft/recraftv2"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(schema.params.contains_key("negative_prompt"), "{id} should declare negative_prompt");
    }
    for id in ["recraft/recraftv4_1", "recraft/recraftv4_1_pro"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            !schema.params.contains_key("negative_prompt"),
            "{id} declares negative_prompt, which Recraft documents for V2 / V3 models only"
        );
    }
}

/// `text_layout` is "V3 models only".
/// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> (Generate image → Parameters), read 2026-09-11
#[test]
fn recraft_text_layout_is_only_allowlisted_on_v3() {
    let reg = registry();
    for m in reg.for_provider("recraft") {
        if m.extra_allowlist.iter().any(|k| k == "text_layout") {
            assert!(
                m.id.starts_with("recraft/recraftv3"),
                "{} allowlists text_layout, a V3-only parameter",
                m.id
            );
        }
    }
}

/// V4.1 takes styles through `style_id` / style references only; the curated
/// `style` names (and substyles) are the V2 / V3 system. The adapter already
/// never forwards `style` to V4.x, so advertising it promised a no-op.
/// @see <https://www.recraft.ai/docs/api-reference/models/recraft-v4-1.md> ("Styles: Apply via style_id or attached style references")
/// @see <https://www.recraft.ai/docs/api-reference/styles.md>, read 2026-09-11
#[test]
fn recraft_v4_1_declares_no_curated_style() {
    let reg = registry();
    for id in ["recraft/recraftv4_1", "recraft/recraftv4_1_pro"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(!schema.params.contains_key("style"), "{id} advertises a curated `style`");
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == "substyle"),
            "{id} allowlists substyle"
        );
        assert!(
            schema.extra_allowlist.iter().any(|k| k == "style_id"),
            "{id} must keep style_id, its only style input"
        );
    }
}

// ─── Vidu: vendor facts re-read 2026-09-11 (model-drift) ────────────────────

/// "3:4 & 4:3 only support q2 & q3 model" (text2video; reference2video says
/// q2), and img2video / start-end2video take no aspect_ratio at all — so q1
/// and 2.0 get 16:9, 9:16 and 1:1 only.
/// @see <https://platform.vidu.com/docs/text-to-video.md>
/// @see <https://platform.vidu.com/docs/reference-to-video.md>, read 2026-09-11
#[test]
fn vidu_3_4_and_4_3_are_only_advertised_for_q2_models() {
    let reg = registry();
    for id in ["vidu/viduq1", "vidu/vidu2.0"] {
        let ratios = aspect_ratios(&reg, id);
        assert!(!ratios.is_empty(), "{id} declares no aspect ratios");
        for ar in ratios {
            assert!(
                ["16:9", "9:16", "1:1"].contains(&ar.as_str()),
                "{id} allows {ar:?}; Vidu's q1 / 2.0 models take 16:9, 9:16 and 1:1 only"
            );
        }
    }
    for ar in aspect_ratios(&reg, "vidu/viduq2-pro") {
        assert!(
            ["16:9", "9:16", "3:4", "4:3", "1:1"].contains(&ar.as_str()),
            "vidu/viduq2-pro allows {ar:?}, not in reference2video's aspect_ratio list"
        );
    }
}

/// `style` (general / anime) exists only on text2video, so only a model that
/// can use text2video may allowlist it (vidu2.0 did, and cannot).
/// @see <https://platform.vidu.com/docs/text-to-video.md>, read 2026-09-11
#[test]
fn vidu_style_is_only_allowlisted_on_text_to_video_models() {
    let reg = registry();
    for m in reg.for_provider("vidu") {
        if m.extra_allowlist.iter().any(|k| k == "style") {
            assert!(
                m.capabilities.text_to_video,
                "{} allowlists text2video's `style` but has no text-to-video mode",
                m.id
            );
        }
    }
}

/// viduq2-pro: 540p/720p/1080p and a 1s floor on img2video, start-end2video and
/// reference2video; start-end2video caps it at 8s (the other two allow 10).
/// @see <https://platform.vidu.com/docs/image-to-video.md>
/// @see <https://platform.vidu.com/docs/start-end-to-video.md>
/// @see <https://platform.vidu.com/docs/reference-to-video.md>, read 2026-09-11
#[test]
fn vidu_q2_pro_duration_and_resolution_hold_on_every_endpoint() {
    let reg = registry();
    let schema = reg.get("vidu/viduq2-pro").expect("viduq2-pro missing");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(1.0), "viduq2-pro accepts 1s clips");
            assert_eq!(f.max, Some(8.0), "start-end2video caps viduq2-pro at 8s");
        }
        other => panic!("viduq2-pro duration_seconds is {other:?}"),
    }
    assert_eq!(
        enum_values(&reg, "vidu/viduq2-pro", "resolution"),
        vec!["540p".to_string(), "720p".to_string(), "1080p".to_string()],
        "viduq2-pro resolutions"
    );
}

// ─── PixVerse: vendor facts re-read 2026-09-11 (model-drift) ────────────────

/// `generate_audio_switch` is "Supported in v5.5/v5.6/v6/c1 models"; v5 and
/// below get audio through sound_effect_* instead.
/// @see <https://docs.platform.pixverse.ai/text-to-video-generation-13016634e0.md>
/// @see <https://docs.platform.pixverse.ai/image-to-video-generation-13016633e0.md>, read 2026-09-11
#[test]
fn pixverse_generate_audio_switch_is_not_allowlisted_below_v5_5() {
    let reg = registry();
    for id in ["pixverse/v3.5", "pixverse/v4.5", "pixverse/v5"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == "generate_audio_switch"),
            "{id} allowlists generate_audio_switch, which PixVerse supports from v5.5 up"
        );
    }
}

// ─── Kling: vendor facts re-read 2026-09-11 (model-drift) ──────────────────

/// Kling's text2video `model_name` enum has no `kling-v2-1`, and its capability
/// map lists Kling 2.1 text-to-video as "Not Supported": the model is
/// image-to-video only, so every request needs its first frame. (The request
/// validator enforces ref roles, not capability flags, so the role is what
/// actually stops a prompt-only request.) Kling retires 2.1 on 2026-09-15; the
/// test tolerates the model's removal.
/// @see https://kling.ai/document-api/api/video/1-6/text-to-video.md (2026-09-11)
#[test]
fn kling_v2_1_video_is_image_to_video_only() {
    let reg = registry();
    if let Some(schema) = reg.get("kling/video-kling-v2-1") {
        assert!(
            !schema.capabilities.text_to_video,
            "kling/video-kling-v2-1 advertises text-to-video; Kling's text2video enum has no kling-v2-1"
        );
        let first = schema
            .ref_inputs
            .as_ref()
            .and_then(|r| r.roles.get("first_frame"))
            .expect("kling/video-kling-v2-1 declares no first_frame role");
        assert!(
            first.required && first.min_count >= 1,
            "kling/video-kling-v2-1 must require its first frame"
        );
    }
}

// ─── Bedrock: vendor facts re-read 2026-09-11 (model-drift) ─────────────────

/// Nova Canvas output resolution: "Each side must be between 320-4096 pixels,
/// inclusive. Each side must be evenly divisible by 16."
/// @see <https://docs.aws.amazon.com/nova/latest/userguide/image-gen-access.html#image-gen-resolutions> (read 2026-09-11)
#[test]
fn bedrock_nova_canvas_sides_are_multiples_of_16() {
    let reg = registry();
    let schema = reg
        .get("bedrock/amazon.nova-canvas-v1:0")
        .expect("nova-canvas missing");
    match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Freeform(f))) => {
            assert_eq!(f.multiple_of, Some(16), "each side must be divisible by 16");
            assert_eq!(
                (f.min_width, f.max_width, f.min_height, f.max_height),
                (320, 4096, 320, 4096),
                "each side is 320-4096 px"
            );
        }
        other => panic!("nova-canvas size is {other:?}, expected a freeform box"),
    }
}

/// The adapter only emits `taskType: TEXT_VIDEO`, whose `text` "Must be 1-512
/// characters in length" (4000 is MULTI_SHOT_AUTOMATED's limit).
/// @see <https://docs.aws.amazon.com/nova/latest/userguide/video-gen-access.html> (read 2026-09-11)
#[test]
fn bedrock_nova_reel_prompt_limit_is_the_text_video_limit() {
    let reg = registry();
    let schema = reg
        .get("bedrock/amazon.nova-reel-v1:1")
        .expect("nova-reel missing");
    assert_eq!(
        schema.prompt.max_length,
        Some(512),
        "TEXT_VIDEO text is 1-512 characters"
    );
}

// ─── Luma: vendor facts re-read 2026-09-11 (model-drift) ────────────────────

/// The three video rows that resolve to a Dream Machine model we can call
/// (luma/dream-machine is sent as ray-2).
const LUMA_DREAM_MACHINE_VIDEO: [&str; 3] = ["luma/dream-machine", "luma/ray-2", "luma/ray-flash-2"];

/// `VideoModelOutputDuration`: "5s" | "9s" — 3s is not a value, and a 3.0
/// floor let `"3s"` through to the vendor.
/// @see <https://docs.lumalabs.ai/reference/creategeneration> (read 2026-09-11)
#[test]
fn luma_video_duration_bounds_are_the_documented_values() {
    let reg = registry();
    for id in LUMA_DREAM_MACHINE_VIDEO {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Float(f)) => {
                assert_eq!(f.min, Some(5.0), "{id}: the shortest Luma duration is 5s");
                assert_eq!(f.max, Some(9.0), "{id}: the longest Luma duration is 9s");
            }
            other => panic!("{id} duration_seconds is {other:?}"),
        }
    }
}

/// `VideoModelOutputResolution`: 540p | 720p | 1080p | 4k, for both ray-2 and
/// ray-flash-2 ("possible values for resolution: 540p, 720p, 1080p, 4k").
/// @see <https://docs.lumalabs.ai/reference/creategeneration> (read 2026-09-11)
/// @see <https://docs.lumalabs.ai/changelog/upscale>
#[test]
fn luma_video_resolutions_are_the_documented_enum() {
    let reg = registry();
    for id in LUMA_DREAM_MACHINE_VIDEO {
        let mut got = enum_values(&reg, id, "resolution");
        got.sort();
        let mut want: Vec<String> = ["540p", "720p", "1080p", "4k"].iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "{id} resolution values");
    }
}

/// `AspectRatio` is one enum for every video model:
/// 1:1, 16:9, 9:16, 4:3, 3:4, 21:9, 9:21.
/// @see <https://docs.lumalabs.ai/reference/creategeneration> (read 2026-09-11)
#[test]
fn luma_video_aspect_ratios_are_the_full_documented_enum() {
    let reg = registry();
    for id in LUMA_DREAM_MACHINE_VIDEO {
        let mut got = aspect_ratios(&reg, id);
        got.sort();
        let mut want: Vec<String> = ["1:1", "16:9", "9:16", "4:3", "3:4", "21:9", "9:21"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        want.sort();
        assert_eq!(got, want, "{id} aspect ratios");
    }
}

// ─── OpenAI: gpt-image-2 size bounds, re-read 2026-09-11 (model-drift) ─────

/// gpt-image-2 `size`: max edge <= 3840, both edges multiples of 16,
/// long:short <= 3:1, and 655,360..=8,294,400 total pixels; `2160x3840`
/// (4K portrait) is a listed size. The catalog capped height at 2160, which
/// rejected that size, and floored edges at 256, where no 3:1 size reaches the
/// pixel minimum (480 is the smallest edge that can: 480x1440 = 691,200 px).
/// @see <https://developers.openai.com/api/docs/guides/image-generation> ("GPT Image 2 settings", 2026-09-11)
#[test]
fn openai_gpt_image_2_size_bounds_match_the_documented_constraints() {
    let reg = registry();
    let schema = reg.get("openai/gpt-image-2").expect("openai/gpt-image-2 missing");
    match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Freeform(f))) => {
            assert_eq!(f.max_width, 3840, "max edge is 3840: {f:?}");
            assert_eq!(f.max_height, 3840, "2160x3840 is a listed size: {f:?}");
            assert_eq!(f.min_width, 480, "no edge under 480 reaches 655,360 px within 3:1: {f:?}");
            assert_eq!(f.min_height, 480, "no edge under 480 reaches 655,360 px within 3:1: {f:?}");
            assert_eq!(f.multiple_of, Some(16), "both edges must be multiples of 16: {f:?}");
        }
        other => panic!("gpt-image-2 size is {other:?}, expected a freeform Size"),
    }
}

// ─── ByteDance: vendor facts re-read 2026-09-11 (model-drift) ───────────────

/// Seedream 4.0 WxH sizes: total pixels in [921600, 16777216] and aspect
/// ratio in [1/16, 16]. Every pair in BytePlus's own 1K/2K/4K size table (and
/// the page's "valid example" 1600x600) must fit the declared box — the old
/// 4096 cap rejected every non-square 4K size.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1541523> (read 2026-09-11)
#[test]
fn bytedance_seedream_4_0_size_box_admits_every_documented_size() {
    const SIZES: [(u32, u32); 25] = [
        (1024, 1024), (1152, 864), (864, 1152), (1280, 720), (720, 1280),
        (1248, 832), (832, 1248), (1512, 648),
        (2048, 2048), (2304, 1728), (1728, 2304), (2848, 1600), (1600, 2848),
        (2496, 1664), (1664, 2496), (3136, 1344),
        (4096, 4096), (3520, 4704), (4704, 3520), (5504, 3040), (3040, 5504),
        (3328, 4992), (4992, 3328), (6240, 2656),
        (1600, 600),
    ];
    let reg = registry();
    let schema = reg
        .get("bytedance/seedream-4-0-250828")
        .expect("seedream-4-0 missing");
    match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Freeform(f))) => {
            for (w, h) in SIZES {
                assert!(
                    (f.min_width..=f.max_width).contains(&w)
                        && (f.min_height..=f.max_height).contains(&h),
                    "seedream-4-0 rejects the documented size {w}x{h} (box {}..={} x {}..={})",
                    f.min_width, f.max_width, f.min_height, f.max_height
                );
            }
        }
        other => panic!("seedream-4-0 size is {other:?}, expected a freeform box"),
    }
}

/// The Image generation API has no `seed` parameter for Seedream 4.0 (seed was
/// a Seedream 3.0 / SeedEdit 3.0 option). Advertising one promises
/// reproducibility the vendor never delivers.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1541523> (read 2026-09-11)
#[test]
fn bytedance_seedream_4_0_declares_no_seed() {
    let reg = registry();
    let schema = reg
        .get("bytedance/seedream-4-0-250828")
        .expect("seedream-4-0 missing");
    assert!(
        !schema.params.contains_key("seed"),
        "seedream-4-0 advertises a seed that the Image generation API does not take"
    );
}

/// "Seedream 5.0 pro supports up to 10 reference images. Seedream 5.0 lite,
/// 4.5, and 4.0 support up to 14 reference images."
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1541523> (read 2026-09-11)
#[test]
fn bytedance_seedream_4_0_accepts_14_reference_images() {
    let reg = registry();
    let ri = reg
        .get("bytedance/seedream-4-0-250828")
        .expect("seedream-4-0 missing")
        .ref_inputs
        .as_ref()
        .expect("seedream-4-0 declares no ref_inputs");
    assert_eq!(ri.max_total, 14, "Seedream 4.0 takes up to 14 reference images");
    assert_eq!(
        ri.roles.get("init").map(|r| r.max_count),
        Some(14),
        "the init role must allow all 14"
    );
}

/// "Dreamina Seedance 1.0 pro: Default 5; supports [2, 12]."
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1520757> (read 2026-09-11)
#[test]
fn bytedance_seedance_1_0_pro_duration_range_matches_the_api() {
    let reg = registry();
    let schema = reg
        .get("bytedance/doubao-seedance-1-0-pro-250528")
        .expect("seedance-1-0-pro missing");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(2.0), "Seedance 1.0 pro accepts 2s");
            assert_eq!(f.max, Some(12.0), "Seedance 1.0 pro tops out at 12s");
            assert_eq!(f.default, Some(5.0), "vendor default is 5s");
        }
        other => panic!("seedance-1-0-pro duration_seconds is {other:?}"),
    }
}

/// `seed` ([-1, 2147483647]) is a documented Seedance 1.0 pro request field;
/// the catalog declared none, so a caller could not ask for a repeatable video.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1520757> (read 2026-09-11)
#[test]
fn bytedance_seedance_1_0_pro_declares_a_seed_param() {
    let reg = registry();
    let schema = reg
        .get("bytedance/doubao-seedance-1-0-pro-250528")
        .expect("seedance-1-0-pro missing");
    match schema.params.get("seed") {
        Some(ParamSpec::Seed(s)) => assert_eq!(s.max, 2_147_483_647, "Seedance seed max"),
        other => panic!("seedance-1-0-pro seed is {other:?}, expected a seed param"),
    }
}

/// Extras are merged into the request body, where the documented field is
/// `camera_fixed`; `camerafixed` was the legacy `--camerafixed` prompt flag and
/// means nothing as a body field.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1520757> (read 2026-09-11)
#[test]
fn bytedance_seedance_1_0_pro_allowlists_the_camera_fixed_body_field() {
    let reg = registry();
    let schema = reg
        .get("bytedance/doubao-seedance-1-0-pro-250528")
        .expect("seedance-1-0-pro missing");
    assert!(
        schema.extra_allowlist.iter().any(|k| k == "camera_fixed"),
        "camera_fixed is the documented body field"
    );
    assert!(
        !schema.extra_allowlist.iter().any(|k| k == "camerafixed"),
        "camerafixed is not a body field"
    );
}

// ─── Leonardo: vendor facts re-read 2026-09-11 (model-drift) ────────────────

/// Motion 2.0 has no duration parameter — the v1 image-to-video `duration`
/// lists values only for VEO3/VEO3FAST/KLING2_5 — and its resolutions are
/// 480p and 720p (RESOLUTION_480 / RESOLUTION_720) only.
/// @see <https://docs.leonardo.ai/v1.0/reference/createimagetovideogeneration> (read 2026-09-11)
/// @see <https://docs.leonardo.ai/docs/motion-20> (read 2026-09-11)
#[test]
fn leonardo_motion2_matches_the_motion_2_0_contract() {
    let reg = registry();
    let schema = reg.get("leonardo/motion2").expect("motion2 missing");
    assert!(
        !schema.params.contains_key("duration_seconds"),
        "Motion 2.0 takes no duration; advertising 4-10s promises a control that does not exist"
    );
    assert_eq!(
        enum_values(&reg, "leonardo/motion2", "resolution"),
        vec!["480p".to_string(), "720p".to_string()],
        "Motion 2.0 renders at 480p or 720p only"
    );
}

/// Kling 2.1 Pro: "resolution ... Set to RESOLUTION_1080", "duration ... Set
/// to 5 or 10"; Leonardo's model schema caps its prompt at 2500 characters.
/// @see <https://docs.leonardo.ai/v1.0/docs/kling-2-1-pro> (read 2026-09-11)
/// @see <https://docs.leonardo.ai/reference/creategeneration> — Kling2_1GenerationRequest, prompt maxLength 2500 (read 2026-09-11)
#[test]
fn leonardo_kling2_1_matches_the_kling_2_1_pro_contract() {
    let reg = registry();
    let schema = reg.get("leonardo/kling2.1").expect("kling2.1 missing");
    assert_eq!(
        enum_values(&reg, "leonardo/kling2.1", "resolution"),
        vec!["1080p".to_string()],
        "Kling 2.1 Pro is 1080p only"
    );
    assert_eq!(schema.prompt.max_length, Some(2500), "Kling 2.1 Pro prompt limit");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(5.0), "Kling 2.1 Pro: 5 or 10 s");
            assert_eq!(f.max, Some(10.0), "Kling 2.1 Pro: 5 or 10 s");
        }
        other => panic!("kling2.1 duration_seconds is {other:?}"),
    }
}

// ─── fal: vendor facts re-read 2026-09-11 (model-drift) ─────────────────────

/// `fal-ai/flux-pro/v1.1` (FLUX1.1 [pro]) takes `prompt`, `image_size`,
/// `num_images`, `seed`, `safety_tolerance`, `output_format`,
/// `enhance_prompt` and `sync_mode` — no `guidance_scale` and no
/// `num_inference_steps`. The catalog advertised both (FLUX.1 [pro] controls),
/// so a caller could set values the endpoint never reads.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-pro/v1.1> (2026-09-11)
#[test]
fn fal_flux_pro_declares_no_steps_or_guidance() {
    let reg = registry();
    let schema = reg.get("fal/flux-pro").expect("fal/flux-pro missing");
    for param in ["steps", "guidance_scale"] {
        assert!(
            !schema.params.contains_key(param),
            "fal/flux-pro declares `{param}`, which fal-ai/flux-pro/v1.1 does not take"
        );
    }
}

/// `fal-ai/flux/dev` `guidance_scale` is 1-20. The catalog allowed values down
/// to 0, which the endpoint rejects.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux/dev> (2026-09-11)
#[test]
fn fal_flux_dev_guidance_scale_is_inside_the_endpoint_range() {
    let reg = registry();
    let schema = reg.get("fal/flux-dev").expect("fal/flux-dev missing");
    match schema.params.get("guidance_scale") {
        Some(ParamSpec::Float(f)) => {
            let min = f.min.expect("fal/flux-dev guidance_scale declares no min");
            let max = f.max.expect("fal/flux-dev guidance_scale declares no max");
            assert!(
                min >= 1.0,
                "guidance_scale min {min} is below fal's minimum of 1"
            );
            assert!(
                max <= 20.0,
                "guidance_scale max {max} is above fal's maximum of 20"
            );
        }
        other => panic!("fal/flux-dev guidance_scale is {other:?}, expected Float"),
    }
}

/// `fal-ai/fast-sdxl` `num_inference_steps` is 1-50 with default 25. The
/// catalog allowed up to 100 (rejected) and defaulted to 50, twice fal's
/// default step count.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/fast-sdxl> (2026-09-11)
#[test]
fn fal_fast_sdxl_steps_match_the_endpoint() {
    let reg = registry();
    let schema = reg.get("fal/sdxl").expect("fal/sdxl missing");
    match schema.params.get("steps") {
        Some(ParamSpec::Int(i)) => {
            assert!(
                i.min.is_some_and(|m| m >= 1),
                "steps min {:?} is below fal's 1",
                i.min
            );
            assert!(
                i.max.is_some_and(|m| m <= 50),
                "steps max {:?} is above fal's 50",
                i.max
            );
            assert_eq!(i.default, Some(25), "fal-ai/fast-sdxl defaults to 25 steps");
        }
        other => panic!("fal/sdxl steps is {other:?}, expected Int"),
    }
}

/// `fal-ai/stable-diffusion-v35-medium` `num_inference_steps` defaults to 40;
/// the catalog said 28.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/stable-diffusion-v35-medium> (2026-09-11)
#[test]
fn fal_sd35_medium_steps_default_is_the_endpoint_default() {
    let reg = registry();
    let schema = reg.get("fal/sd35-medium").expect("fal/sd35-medium missing");
    match schema.params.get("steps") {
        Some(ParamSpec::Int(i)) => {
            assert_eq!(
                i.default,
                Some(40),
                "fal-ai/stable-diffusion-v35-medium defaults to 40 steps"
            );
            assert!(
                i.min.is_some_and(|m| m >= 1),
                "steps min {:?} is below fal's 1",
                i.min
            );
            assert!(
                i.max.is_some_and(|m| m <= 50),
                "steps max {:?} is above fal's 50",
                i.max
            );
        }
        other => panic!("fal/sd35-medium steps is {other:?}, expected Int"),
    }
}

/// Recraft V3 on fal caps `prompt` at 1000 characters (`maxLength: 1000`, on
/// both `fal-ai/recraft-v3` and its successor id
/// `fal-ai/recraft/v3/text-to-image`). The catalog allowed 5000.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/recraft/v3/text-to-image> (2026-09-11)
#[test]
fn fal_recraft_v3_prompt_limit_is_1000_characters() {
    let reg = registry();
    let schema = reg.get("fal/recraft-v3").expect("fal/recraft-v3 missing");
    assert_eq!(
        schema.prompt.max_length,
        Some(1000),
        "fal's Recraft V3 prompt maxLength is 1000"
    );
}

/// `fal-ai/aura-flow` `num_inference_steps` is 20-50 with default 50. The
/// catalog allowed 10-19 (rejected) and defaulted to 25.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/aura-flow> (2026-09-11)
#[test]
fn fal_aura_flow_steps_match_the_endpoint() {
    let reg = registry();
    let schema = reg.get("fal/auraflow").expect("fal/auraflow missing");
    match schema.params.get("steps") {
        Some(ParamSpec::Int(i)) => {
            assert!(
                i.min.is_some_and(|m| m >= 20),
                "steps min {:?} is below fal's 20",
                i.min
            );
            assert!(
                i.max.is_some_and(|m| m <= 50),
                "steps max {:?} is above fal's 50",
                i.max
            );
            assert_eq!(i.default, Some(50), "fal-ai/aura-flow defaults to 50 steps");
        }
        other => panic!("fal/auraflow steps is {other:?}, expected Int"),
    }
}

/// `fal/video` runs `fal-ai/ltx-video` (text) or `fal-ai/ltx-video/image-to-video`
/// (image). Neither input schema has a duration or frame-count field, and the
/// adapter never forwards `duration_seconds` to them, so a declared 2-10 s
/// range promised a control that does nothing.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-video> (2026-09-11)
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-video/image-to-video> (2026-09-11)
#[test]
fn fal_video_does_not_advertise_a_duration() {
    let reg = registry();
    let schema = reg.get("fal/video").expect("fal/video missing");
    assert!(
        !schema.params.contains_key("duration_seconds"),
        "fal/video declares duration_seconds, but LTX Video on fal takes no duration"
    );
}

// ─── Replicate: vendor facts re-read 2026-09-11 (model-drift) ───────────────

/// `black-forest-labs/flux-pro` `aspect_ratio` enum is
/// `custom, 1:1, 16:9, 3:2, 2:3, 4:5, 5:4, 9:16, 3:4, 4:3` — no `21:9` or
/// `9:21` (flux-dev and flux-schnell have those; flux-pro does not).
/// @see <https://replicate.com/black-forest-labs/flux-pro/api/schema> (2026-09-11)
#[test]
fn replicate_flux_pro_aspect_ratios_are_in_the_flux_pro_enum() {
    const FLUX_PRO_RATIOS: [&str; 9] = [
        "1:1", "16:9", "3:2", "2:3", "4:5", "5:4", "9:16", "3:4", "4:3",
    ];
    let reg = registry();
    let allowed = aspect_ratios(&reg, "replicate/flux-pro");
    assert!(
        !allowed.is_empty(),
        "replicate/flux-pro declares no aspect ratios"
    );
    for ar in allowed {
        assert!(
            FLUX_PRO_RATIOS.contains(&ar.as_str()),
            "replicate/flux-pro allows {ar:?}, not in black-forest-labs/flux-pro's aspect_ratio enum"
        );
    }
}

/// `black-forest-labs/flux-pro` takes guidance as `guidance`, 2-5 (default
/// 3). The catalog allowed down to 1.5.
/// @see <https://replicate.com/black-forest-labs/flux-pro/api/schema> (2026-09-11)
#[test]
fn replicate_flux_pro_guidance_is_inside_the_guidance_input_range() {
    let reg = registry();
    let schema = reg
        .get("replicate/flux-pro")
        .expect("replicate/flux-pro missing");
    match schema.params.get("guidance_scale") {
        Some(ParamSpec::Float(f)) => {
            let min = f
                .min
                .expect("replicate/flux-pro guidance_scale declares no min");
            let max = f
                .max
                .expect("replicate/flux-pro guidance_scale declares no max");
            assert!(
                min >= 2.0,
                "guidance min {min} is below flux-pro's minimum of 2"
            );
            assert!(
                max <= 5.0,
                "guidance max {max} is above flux-pro's maximum of 5"
            );
        }
        other => panic!("replicate/flux-pro guidance_scale is {other:?}, expected Float"),
    }
}

/// `black-forest-labs/flux-pro` marks `steps` (and `interval`)
/// `"deprecated": true`, described only as "Deprecated". Advertising steps
/// offers a control the model no longer honours.
/// @see <https://replicate.com/black-forest-labs/flux-pro/api/schema> (2026-09-11)
#[test]
fn replicate_flux_pro_does_not_advertise_the_deprecated_steps() {
    let reg = registry();
    let schema = reg
        .get("replicate/flux-pro")
        .expect("replicate/flux-pro missing");
    assert!(
        !schema.params.contains_key("steps"),
        "replicate/flux-pro declares steps, which black-forest-labs/flux-pro marks deprecated"
    );
}

/// `stability-ai/stable-diffusion-3` takes step count as `steps`, 1-28. The
/// catalog allowed up to 50.
/// @see <https://replicate.com/stability-ai/stable-diffusion-3/api/schema> (2026-09-11)
#[test]
fn replicate_sd3_steps_stay_inside_the_steps_input_range() {
    let reg = registry();
    let schema = reg.get("replicate/sd3").expect("replicate/sd3 missing");
    match schema.params.get("steps") {
        Some(ParamSpec::Int(i)) => {
            assert!(
                i.min.is_some_and(|m| m >= 1),
                "steps min {:?} is below SD3's 1",
                i.min
            );
            assert!(
                i.max.is_some_and(|m| m <= 28),
                "steps max {:?} is above SD3's 28",
                i.max
            );
        }
        other => panic!("replicate/sd3 steps is {other:?}, expected Int"),
    }
}

/// `stability-ai/stable-diffusion-3` takes guidance as `cfg`, 0-20, default
/// 3.5. The catalog's default was 4.5.
/// @see <https://replicate.com/stability-ai/stable-diffusion-3/api/schema> (2026-09-11)
#[test]
fn replicate_sd3_guidance_default_is_the_cfg_default() {
    let reg = registry();
    let schema = reg.get("replicate/sd3").expect("replicate/sd3 missing");
    match schema.params.get("guidance_scale") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.default, Some(3.5), "SD3's cfg defaults to 3.5");
            assert!(
                f.min.is_some_and(|m| m >= 0.0),
                "cfg min {:?} is below 0",
                f.min
            );
            assert!(
                f.max.is_some_and(|m| m <= 20.0),
                "cfg max {:?} is above 20",
                f.max
            );
        }
        other => panic!("replicate/sd3 guidance_scale is {other:?}, expected Float"),
    }
}

/// `replicate/video` runs `lucataco/animate-diff` at a pinned version whose
/// input is exactly `prompt, n_prompt, seed, steps, guidance_scale,
/// motion_module, path` — there is no image input. The catalog advertised
/// image-to-video with an `init` ref, which the adapter sent as `init_image`
/// and the model ignored.
/// @see <https://replicate.com/lucataco/animate-diff/versions/beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f> (2026-09-11)
#[test]
fn replicate_video_is_text_to_video_only() {
    let reg = registry();
    let schema = reg.get("replicate/video").expect("replicate/video missing");
    assert!(
        !schema.capabilities.image_to_video,
        "replicate/video advertises image_to_video, but AnimateDiff has no image input"
    );
    assert!(
        schema.ref_inputs.is_none(),
        "replicate/video declares ref_inputs, but AnimateDiff has no image input"
    );
    assert!(schema.capabilities.text_to_video);
}

/// The pinned AnimateDiff version has no duration, fps or frame-count input
/// (its clip length is fixed), and the adapter never forwards
/// `duration_seconds`, so a declared 2-10 s range promised a control that does
/// nothing.
/// @see <https://replicate.com/lucataco/animate-diff/versions/beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f> (2026-09-11)
#[test]
fn replicate_video_does_not_advertise_a_duration() {
    let reg = registry();
    let schema = reg.get("replicate/video").expect("replicate/video missing");
    assert!(
        !schema.params.contains_key("duration_seconds"),
        "replicate/video declares duration_seconds, but AnimateDiff takes no duration"
    );
}

/// `extra` keys are merged into the prediction's `input`, so every allowlisted
/// key must be an AnimateDiff input. `version` is not one (a different version
/// is pinned through `model_mapping`).
/// @see <https://replicate.com/lucataco/animate-diff/versions/beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f> (2026-09-11)
#[test]
fn replicate_video_extra_allowlist_names_only_animate_diff_inputs() {
    const ANIMATE_DIFF_INPUTS: [&str; 7] = [
        "prompt",
        "n_prompt",
        "seed",
        "steps",
        "guidance_scale",
        "motion_module",
        "path",
    ];
    let reg = registry();
    let schema = reg.get("replicate/video").expect("replicate/video missing");
    for key in &schema.extra_allowlist {
        assert!(
            ANIMATE_DIFF_INPUTS.contains(&key.as_str()),
            "replicate/video allowlists extra.{key}, which AnimateDiff's input does not declare"
        );
    }
}

// ─── Stability: vendor facts re-read 2026-09-11 (model-drift) ───────────────

/// SDXL 1.0 (`stable-diffusion-xl-1024-v1-0`) text-to-image takes `height` &
/// `width` only as one of 1024x1024, 1152x896, 896x1152, 1216x832, 1344x768,
/// 768x1344, 1536x640, 640x1536. The catalog offered 512x512 — not in the set,
/// and the Playground's default for this model because it was listed first.
/// @see <https://platform.stability.ai/docs/api-reference#tag/SDXL-1.0> (2026-09-11)
#[test]
fn stability_sdxl_sizes_are_the_sdxl_1_0_dimension_set() {
    const SDXL_1_0_SIZES: [(u32, u32); 8] = [
        (1024, 1024),
        (1152, 896),
        (896, 1152),
        (1216, 832),
        (1344, 768),
        (768, 1344),
        (1536, 640),
        (640, 1536),
    ];
    let reg = registry();
    let schema = reg.get("stability/sdxl").expect("stability/sdxl missing");
    match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Enum(e))) => {
            for wh in &e.values {
                assert!(
                    SDXL_1_0_SIZES.contains(wh),
                    "stability/sdxl allows {wh:?}, which the SDXL 1.0 engine rejects"
                );
            }
            for wh in &SDXL_1_0_SIZES {
                assert!(
                    e.values.contains(wh),
                    "stability/sdxl omits {wh:?}, a documented SDXL 1.0 size"
                );
            }
            assert_eq!(
                e.values.first(),
                Some(&(1024, 1024)),
                "1024x1024 is the documented default and the first-listed size is the Playground default"
            );
        }
        other => panic!("stability/sdxl size is {other:?}, expected an enum Size"),
    }
}

/// `/v2beta/stable-image/generate/sd3`'s `aspect_ratio` enum
/// (21:9, 16:9, 3:2, 5:4, 1:1, 4:5, 2:3, 9:16, 9:21) applies to every `model`
/// value, `sd3.5-large-turbo` included; the route documents no per-model
/// restriction. The catalog offered sd3-turbo 1:1 only.
/// @see <https://api.stability.ai/v2alpha/openapi> (2026-09-11)
#[test]
fn stability_sd3_turbo_advertises_every_sd3_aspect_ratio() {
    const V2_RATIOS: [&str; 9] = [
        "21:9", "16:9", "3:2", "5:4", "1:1", "4:5", "2:3", "9:16", "9:21",
    ];
    let reg = registry();
    let allowed = aspect_ratios(&reg, "stability/sd3-turbo");
    for ar in V2_RATIOS {
        assert!(
            allowed.iter().any(|a| a == ar),
            "stability/sd3-turbo omits {ar:?}, which /generate/sd3 accepts for sd3.5-large-turbo"
        );
    }
}

// ─── bytedance: 2026-09-11 drift decisions applied ──────────────────────────

/// Seedream 5.0 lite and 4.5 (the vendor's successors to the deactivated
/// Seedream 3.0) take text-to-image and up to 14 reference images, have no
/// `seed` (the Image generation API documents none), and their WxH sizes need
/// total pixels >= 3686400 (2560x1440) at an aspect ratio in [1/16, 16] — so no
/// side can be under sqrt(3686400/16) = 480, a stricter floor than 4.0's 240.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1541523> (2026-09-11)
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1330310> (2026-09-11)
#[test]
fn bytedance_seedream_5_0_lite_and_4_5_are_advertised_with_their_size_floor() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    for id in ["bytedance/seedream-5-0-lite-260128", "bytedance/seedream-4-5-251128"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing from the catalog"));
        assert!(
            schema.capabilities.text_to_image && schema.capabilities.image_to_image,
            "{id}: text-to-image and (multi-)reference image-to-image"
        );
        assert!(
            schema.params.get("seed").is_none(),
            "{id}: the Image generation API has no seed parameter"
        );
        match schema.params.get("size") {
            Some(ParamSpec::Size(SizeSpec::Freeform(f))) => {
                assert_eq!((f.min_width, f.min_height), (480, 480), "{id}: pixel floor 3686400 at ratio 1/16..16");
                assert_eq!((f.max_width, f.max_height), (16384, 16384), "{id}: pixel cap 16777216 at ratio 1/16..16");
            }
            other => panic!("{id} size is {other:?}, expected a freeform WxH size"),
        }
        let refs = schema.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
        assert_eq!(refs.max_total, 14, "{id}: up to 14 reference images");
    }
}

/// Seedance 1.0 Pro Fast, the vendor's successor to the deactivated Seedance
/// 1.0 Lite: text-to-video and first-frame image-to-video only (no
/// first-and-last-frame mode), duration [2, 12] s, resolution 480p/720p/1080p.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1520757> (2026-09-11)
#[test]
fn bytedance_seedance_1_0_pro_fast_is_first_frame_only() {
    let id = "bytedance/seedance-1-0-pro-fast-251015";
    let reg = registry();
    let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(schema.capabilities.text_to_video && schema.capabilities.image_to_video);
    let roles = &schema.ref_inputs.as_ref().expect("seedance-1-0-pro-fast declares no ref_inputs").roles;
    assert!(roles.contains_key("first_frame"), "first-frame image-to-video is supported");
    assert!(
        !roles.contains_key("last_frame"),
        "1.0 pro fast has no first-and-last-frame mode; a last_frame role promises one"
    );
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => assert_eq!((f.min, f.max), (Some(2.0), Some(12.0)), "duration [2, 12]"),
        other => panic!("{id} duration_seconds is {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "resolution"), ["480p", "720p", "1080p"]);
}

/// Dreamina Seedance 2.0 mini: text-to-video plus first / first-and-last-frame
/// image-to-video, 480p/720p only, duration [4, 15] s, and no `seed` (seed is
/// documented for Seedance 1.5 pro / 1.0 pro / 1.0 pro fast only). The 2.0
/// series' omni-reference roles are not advertised: the adapter only sends
/// image_url blocks tagged first_frame / last_frame.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1520757> (2026-09-11)
#[test]
fn bytedance_seedance_2_0_mini_advertises_only_frame_roles_and_its_bounds() {
    let id = "bytedance/dreamina-seedance-2-0-mini-260615";
    let reg = registry();
    let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(schema.capabilities.text_to_video && schema.capabilities.image_to_video);
    let mut roles: Vec<&str> = schema
        .ref_inputs
        .as_ref()
        .expect("seedance-2-0-mini declares no ref_inputs")
        .roles
        .keys()
        .map(String::as_str)
        .collect();
    roles.sort_unstable();
    assert_eq!(roles, ["first_frame", "last_frame"], "no reference_image/_video/_audio roles");
    assert_eq!(enum_values(&reg, id, "resolution"), ["480p", "720p"], "2.0 mini has no 1080p/4k");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => assert_eq!((f.min, f.max), (Some(4.0), Some(15.0)), "duration [4, 15]"),
        other => panic!("{id} duration_seconds is {other:?}"),
    }
    assert!(schema.params.get("seed").is_none(), "seed is not a 2.0-series parameter");
}

// ─── kling: 2026-09-11 drift decisions applied ──────────────────────────────

/// Kling Image 2.1 on POST /v1/images/generations (`model_name` kling-v2-1):
/// text + image input, eight aspect ratios including 21:9, prompt at most
/// 2500 characters; `image_fidelity` is kling-v1/v1-5 only.
/// @see https://kling.ai/document-api/api/image/2-1/image-generation.md (2026-09-11)
/// @see https://kling.ai/document-api/guides/capability-map/image.md (2026-09-11)
#[test]
fn kling_image_2_1_matches_the_image_2_1_contract() {
    let reg = registry();
    let schema = reg.get("kling/kling-v2-1").expect("kling/kling-v2-1 missing");
    assert!(schema.capabilities.text_to_image && schema.capabilities.image_to_image);
    assert_eq!(schema.prompt.max_length, Some(2500), "Image 2.1 prompt limit");
    let mut got = aspect_ratios(&reg, "kling/kling-v2-1");
    got.sort();
    let mut want: Vec<String> = ["16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "21:9"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    want.sort();
    assert_eq!(got, want, "Image 2.1 aspect ratios");
    assert!(
        !schema.extra_allowlist.iter().any(|k| k == "image_fidelity" || k == "human_fidelity"),
        "image_fidelity / human_fidelity are kling-v1/v1-5 only"
    );
}

/// Kling 2.5 Turbo and 2.6 on the legacy text2video / image2video endpoints:
/// text and image input, 5 or 10 s (advertised as a 5-10 range), text2video
/// aspect ratios 16:9 | 9:16 | 1:1. cfg_scale ("kling-v2.x models do not
/// support this parameter") and camera_control ("Not Supported") must not be
/// allowlisted; `sound` (native audio) is a 2.6 capability only.
/// @see https://kling.ai/document-api/api/video/1-6/text-to-video.md (2026-09-11)
/// @see https://kling.ai/document-api/api/video/2-1/image-to-video.md (2026-09-11)
/// @see https://kling.ai/document-api/guides/capability-map/video.md (2026-09-11)
#[test]
fn kling_v2_5_turbo_and_v2_6_video_match_the_capability_map() {
    let reg = registry();
    for id in ["kling/video-kling-v2-5-turbo", "kling/video-kling-v2-6"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            schema.capabilities.text_to_video && schema.capabilities.image_to_video,
            "{id}: text and image input"
        );
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Float(f)) => {
                assert_eq!(f.min, Some(5.0), "{id}: 5 or 10 s");
                assert_eq!(f.max, Some(10.0), "{id}: 5 or 10 s");
            }
            other => panic!("{id} duration_seconds is {other:?}"),
        }
        let mut got = aspect_ratios(&reg, id);
        got.sort();
        let mut want: Vec<String> = ["16:9", "9:16", "1:1"].iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(got, want, "{id} aspect ratios");
        for key in ["cfg_scale", "camera_control"] {
            assert!(
                !schema.extra_allowlist.iter().any(|k| k == key),
                "{id} allowlists {key}, which Kling 2.5 Turbo / 2.6 do not support"
            );
        }
        assert!(schema.extra_allowlist.iter().any(|k| k == "mode"), "{id}: mode selects 720p/1080p");
    }
    let has_sound = |id: &str| reg.get(id).unwrap().extra_allowlist.iter().any(|k| k == "sound");
    assert!(has_sound("kling/video-kling-v2-6"), "Kling 2.6 documents native audio (sound)");
    assert!(!has_sound("kling/video-kling-v2-5-turbo"), "Kling 2.5 Turbo has no native audio");
}

// ─── hunyuan: 2026-09-11 drift decisions applied ────────────────────────────

/// SubmitHunyuanToVideoJob generates "根据输入的文本或图片" (from text or an
/// image): Prompt is its only required member and Image is optional, so the
/// row is text- and image-to-video and must not require a first frame.
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/client.go> (2026-09-11)
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-cli/master/tccli/services/vclm/v20240523/api.json> (2026-09-11)
#[test]
fn hunyuan_video_is_text_and_image_to_video() {
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-video").expect("hunyuan-video missing");
    assert!(schema.capabilities.text_to_video, "SubmitHunyuanToVideoJob takes a prompt alone");
    assert!(schema.capabilities.image_to_video, "SubmitHunyuanToVideoJob takes an optional Image");
    let first = schema
        .ref_inputs
        .as_ref()
        .and_then(|r| r.roles.get("first_frame"))
        .expect("hunyuan-video declares no first_frame role");
    assert!(!first.required && first.min_count == 0, "Image is optional");
    assert_eq!(first.max_count, 1, "Image is a single image");
}

/// `SubmitHunyuanToVideoJob.Prompt`: "最多支持200个 utf-8 字符" (at most 200 characters).
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/models.go> (2026-09-11)
#[test]
fn hunyuan_video_prompt_limit_matches_submit_hunyuan_to_video_job() {
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-video").expect("hunyuan-video missing");
    assert_eq!(schema.prompt.max_length, Some(200), "SubmitHunyuanToVideoJob caps Prompt at 200 characters");
}

/// `SubmitHunyuanToVideoJob.Resolution`: "目前仅支持720p视频分辨率，默认720p" (720p only).
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/models.go> (2026-09-11)
#[test]
fn hunyuan_video_resolution_is_720p_only() {
    let reg = registry();
    assert_eq!(
        enum_values(&reg, "hunyuan/hunyuan-video", "resolution"),
        vec!["720p".to_string()],
        "SubmitHunyuanToVideoJob supports 720p only"
    );
}

/// The adapter merges every allowlisted extra key into the request verbatim,
/// and Tencent fails an undefined parameter (UnknownParameter).
/// SubmitHunyuanToVideoJob takes exactly these five.
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/models.go> (2026-09-11)
#[test]
fn hunyuan_video_extra_allowlist_names_only_submit_hunyuan_to_video_job_parameters() {
    const PARAMS: [&str; 5] = ["Prompt", "Image", "Resolution", "LogoAdd", "LogoParam"];
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-video").expect("hunyuan-video missing");
    for key in &schema.extra_allowlist {
        assert!(
            PARAMS.contains(&key.as_str()),
            "hunyuan-video allowlists {key:?}, which is not a SubmitHunyuanToVideoJob parameter"
        );
    }
}

// ─── leonardo: 2026-09-11 drift decisions applied ───────────────────────────

/// Veo 3.1 (`VEO3_1`): "duration ... Set to 4, 6, or 8", "resolution ... Set
/// to RESOLUTION_720 and RESOLUTION_1080".
/// @see <https://docs.leonardo.ai/v1.0/docs/veo-31> (read 2026-09-11)
#[test]
fn leonardo_veo3_1_matches_the_veo_3_1_contract() {
    let reg = registry();
    let schema = reg.get("leonardo/veo3.1").expect("veo3.1 missing");
    assert!(schema.capabilities.image_to_video);
    assert_eq!(
        enum_values(&reg, "leonardo/veo3.1", "resolution"),
        vec!["720p".to_string(), "1080p".to_string()],
        "Veo 3.1 renders at 720p or 1080p"
    );
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(4.0), "Veo 3.1: 4, 6 or 8 s");
            assert_eq!(f.max, Some(8.0), "Veo 3.1: 4, 6 or 8 s");
        }
        other => panic!("veo3.1 duration_seconds is {other:?}"),
    }
}

/// Kling 2.5 Turbo (`KLING2_5`): "resolution ... Set to RESOLUTION_1080",
/// "duration ... Set to 5 or 10"; Leonardo's model schema caps its prompt at
/// 2500 characters.
/// @see <https://docs.leonardo.ai/v1.0/docs/kling-2-5-turbo> (read 2026-09-11)
/// @see <https://docs.leonardo.ai/reference/creategeneration> — Kling2_5GenerationRequest, prompt maxLength 2500 (read 2026-09-11)
#[test]
fn leonardo_kling2_5_matches_the_kling_2_5_turbo_contract() {
    let reg = registry();
    let schema = reg.get("leonardo/kling2.5").expect("kling2.5 missing");
    assert!(schema.capabilities.image_to_video);
    assert_eq!(
        enum_values(&reg, "leonardo/kling2.5", "resolution"),
        vec!["1080p".to_string()],
        "Kling 2.5 Turbo is 1080p only"
    );
    assert_eq!(schema.prompt.max_length, Some(2500), "Kling 2.5 Turbo prompt limit");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(5.0), "Kling 2.5 Turbo: 5 or 10 s");
            assert_eq!(f.max, Some(10.0), "Kling 2.5 Turbo: 5 or 10 s");
        }
        other => panic!("kling2.5 duration_seconds is {other:?}"),
    }
}

// ─── vidu: 2026-09-11 drift decisions applied ───────────────────────────────

/// viduq3-turbo is in the `model` values of all four endpoints the adapter
/// routes between (text2video, img2video, start-end2video, reference2video),
/// so it advertises both modes and all three ref roles. reference2video takes
/// 3-16s (the others 1-16s), so 3-16s is the range every endpoint accepts.
/// @see <https://platform.vidu.com/docs/model-map.md>
/// @see <https://platform.vidu.com/docs/reference-to-video.md>, read 2026-09-11
#[test]
fn vidu_q3_turbo_serves_every_endpoint_the_adapter_routes_to() {
    let reg = registry();
    let schema = reg.get("vidu/viduq3-turbo").expect("vidu/viduq3-turbo missing");
    assert!(schema.capabilities.text_to_video, "viduq3-turbo is a text2video model");
    assert!(schema.capabilities.image_to_video, "viduq3-turbo is an img2video model");
    let roles = &schema
        .ref_inputs
        .as_ref()
        .expect("vidu/viduq3-turbo declares no ref_inputs")
        .roles;
    for role in ["first_frame", "last_frame", "reference"] {
        assert!(roles.contains_key(role), "vidu/viduq3-turbo is missing the {role} role");
    }
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(3.0), "reference2video takes viduq3-turbo from 3s");
            assert_eq!(f.max, Some(16.0), "q3 models go to 16s");
        }
        other => panic!("viduq3-turbo duration_seconds is {other:?}"),
    }
    assert_eq!(
        enum_values(&reg, "vidu/viduq3-turbo", "resolution"),
        vec!["540p".to_string(), "720p".to_string(), "1080p".to_string()],
        "viduq3-turbo resolutions"
    );
}

/// viduq3-pro is accepted by text2video, img2video and start-end2video, but
/// by neither reference2video `model` list, so it must not declare a
/// `reference` role (the adapter sends any reference ref to reference2video).
/// @see <https://platform.vidu.com/docs/reference-to-video.md>
/// @see <https://platform.vidu.com/docs/start-end-to-video.md>, read 2026-09-11
#[test]
fn vidu_q3_pro_has_no_reference_to_video_mode() {
    let reg = registry();
    let schema = reg.get("vidu/viduq3-pro").expect("vidu/viduq3-pro missing");
    assert!(schema.capabilities.text_to_video, "viduq3-pro is a text2video model");
    assert!(schema.capabilities.image_to_video, "viduq3-pro is an img2video model");
    let roles = &schema
        .ref_inputs
        .as_ref()
        .expect("vidu/viduq3-pro declares no ref_inputs")
        .roles;
    assert!(roles.contains_key("first_frame") && roles.contains_key("last_frame"));
    assert!(
        !roles.contains_key("reference"),
        "vidu/viduq3-pro declares a reference role; reference2video does not accept viduq3-pro"
    );
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(1.0), "viduq3-pro accepts 1s clips");
            assert_eq!(f.max, Some(16.0), "q3 models go to 16s");
        }
        other => panic!("viduq3-pro duration_seconds is {other:?}"),
    }
}

/// q3 models take `audio` (native audio-video, default true) and do not take
/// `bgm` ("BGM does not available in q3 models").
/// @see <https://platform.vidu.com/docs/text-to-video.md>
/// @see <https://platform.vidu.com/docs/image-to-video.md>, read 2026-09-11
#[test]
fn vidu_q3_models_allowlist_audio_but_not_bgm() {
    let reg = registry();
    for id in ["vidu/viduq3-turbo", "vidu/viduq3-pro"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            schema.extra_allowlist.iter().any(|k| k == "audio"),
            "{id} extra_allowlist is missing audio: {:?}",
            schema.extra_allowlist
        );
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == "bgm"),
            "{id} allowlists bgm, which q3 models do not support"
        );
    }
}

/// viduq2-pro-fast is only in img2video's and start-end2video's `model`
/// values: no text-to-video, no reference role, a required start frame, and
/// no aspect_ratio (neither endpoint takes one). 720p / 1080p only, and 1-8s
/// (start-end2video's ceiling; img2video goes to 10s).
/// @see <https://platform.vidu.com/docs/image-to-video.md>
/// @see <https://platform.vidu.com/docs/start-end-to-video.md>, read 2026-09-11
#[test]
fn vidu_q2_pro_fast_is_image_to_video_only() {
    let reg = registry();
    let schema = reg.get("vidu/viduq2-pro-fast").expect("vidu/viduq2-pro-fast missing");
    assert!(
        !schema.capabilities.text_to_video,
        "text2video does not accept viduq2-pro-fast"
    );
    assert!(schema.capabilities.image_to_video);
    let ri = schema
        .ref_inputs
        .as_ref()
        .expect("vidu/viduq2-pro-fast declares no ref_inputs");
    let first = ri.roles.get("first_frame").expect("viduq2-pro-fast declares no first_frame role");
    assert!(
        first.required && first.min_count == 1,
        "both viduq2-pro-fast endpoints need the start frame: {first:?}"
    );
    assert!(
        !ri.roles.contains_key("reference"),
        "reference2video does not accept viduq2-pro-fast"
    );
    assert!(
        aspect_ratios(&reg, "vidu/viduq2-pro-fast").is_empty(),
        "img2video / start-end2video take no aspect_ratio"
    );
    assert_eq!(
        enum_values(&reg, "vidu/viduq2-pro-fast", "resolution"),
        vec!["720p".to_string(), "1080p".to_string()],
        "viduq2-pro-fast resolutions (no 540p)"
    );
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(1.0), "viduq2-pro-fast accepts 1s clips");
            assert_eq!(f.max, Some(8.0), "start-end2video caps viduq2-pro-fast at 8s");
        }
        other => panic!("viduq2-pro-fast duration_seconds is {other:?}"),
    }
}

// ─── pixverse: 2026-09-11 drift decisions applied ───────────────────────────

/// V6 and C1 (PixVerse's two recommended models) serve text/, img/ and
/// transition/generate with "v6/c1 : 1~15" second durations, the extended
/// aspect-ratio list "16:9, 4:3, 1:1, 3:4, 9:16, 2:3, 3:2, 21:9", and
/// `generate_audio_switch` ("Supported in v5.5/v5.6/v6/c1 models").
/// @see <https://docs.platform.pixverse.ai/text-to-video-generation-13016634e0.md>
/// @see <https://docs.platform.pixverse.ai/capability-matrix-2144288m0.md>, read 2026-09-11
#[test]
fn pixverse_v6_and_c1_are_carried_with_their_documented_contract() {
    let reg = registry();
    for id in ["pixverse/v6", "pixverse/c1"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(schema.capabilities.text_to_video, "{id} serves text/generate");
        assert!(schema.capabilities.image_to_video, "{id} serves img/generate");
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Float(f)) => {
                assert_eq!(f.min, Some(1.0), "{id} takes 1-15s");
                assert_eq!(f.max, Some(15.0), "{id} takes 1-15s");
            }
            other => panic!("{id} duration_seconds is {other:?}"),
        }
        let ratios = aspect_ratios(&reg, id);
        for ar in ["2:3", "3:2", "21:9"] {
            assert!(
                ratios.contains(&ar.to_string()),
                "{id} does not advertise {ar:?}, a v6/c1 aspect ratio"
            );
        }
        assert!(
            schema.extra_allowlist.iter().any(|k| k == "generate_audio_switch"),
            "{id} extra_allowlist is missing generate_audio_switch: {:?}",
            schema.extra_allowlist
        );
    }
}

/// `generate_multi_clip_switch` is "Supported in v5.5/v6 models"; C1 does
/// multi-clip from the prompt and has no such switch.
/// @see <https://docs.platform.pixverse.ai/text-to-video-generation-13016634e0.md>
/// @see <https://docs.platform.pixverse.ai/capability-matrix-2144288m0.md>, read 2026-09-11
#[test]
fn pixverse_multi_clip_switch_is_allowlisted_on_v6_not_c1() {
    let reg = registry();
    let has = |id: &str| {
        reg.get(id)
            .unwrap_or_else(|| panic!("{id} missing"))
            .extra_allowlist
            .iter()
            .any(|k| k == "generate_multi_clip_switch")
    };
    assert!(has("pixverse/v6"), "pixverse/v6 supports generate_multi_clip_switch");
    assert!(
        !has("pixverse/c1"),
        "pixverse/c1 allowlists generate_multi_clip_switch, which PixVerse documents for v5.5/v6 only"
    );
}

// ─── minimax: 2026-09-11 drift decisions applied ────────────────────────────

/// MiniMax-Hailuo-2.3-Fast is in the image-to-video `model` enum only (not
/// text-to-video, not first & last frame), and the i2v request requires
/// `first_frame_image`. It takes 768P / 1080P (6 or 10 s, 10 s at 768P only)
/// and `fast_pretreatment`.
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-i2v.md>
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-t2v.md>, read 2026-09-11
#[test]
fn minimax_hailuo_2_3_fast_is_image_to_video_only() {
    let reg = registry();
    let id = "minimax/MiniMax-Hailuo-2.3-Fast";
    let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
    assert!(
        !schema.capabilities.text_to_video,
        "{id} advertises text-to-video; the t2v model enum has no MiniMax-Hailuo-2.3-Fast"
    );
    assert!(schema.capabilities.image_to_video);
    let ri = schema.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
    let first = ri.roles.get("first_frame").unwrap_or_else(|| panic!("{id} declares no first_frame role"));
    assert!(
        first.required && first.min_count == 1 && first.max_count == 1,
        "first_frame_image is required, exactly one image: {first:?}"
    );
    assert!(
        !ri.roles.contains_key("last_frame"),
        "{id} declares a last_frame role, but fl2v only accepts MiniMax-Hailuo-02"
    );
    assert_eq!(
        enum_values(&reg, id, "resolution"),
        vec!["768P".to_string(), "1080P".to_string()],
        "{id} resolutions"
    );
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(6.0), "{id} takes 6 or 10 s");
            assert_eq!(f.max, Some(10.0), "{id} takes 6 or 10 s");
        }
        other => panic!("{id} duration_seconds is {other:?}"),
    }
    assert!(
        schema.extra_allowlist.iter().any(|k| k == "fast_pretreatment"),
        "{id} extra_allowlist is missing fast_pretreatment: {:?}",
        schema.extra_allowlist
    );
}

// ─── recraft: 2026-09-11 drift decisions applied ────────────────────────────

/// The three vector (SVG) models are carried, and none declares a pixel
/// `size`: for vector models the Appendix lists aspects only (`w:h`, no WxH),
/// which an enumerated WxH size cannot express.
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("List of supported image sizes"), read 2026-09-11
#[test]
fn recraft_vector_models_are_carried_without_a_pixel_size() {
    let reg = registry();
    for id in [
        "recraft/recraftv4_1_vector",
        "recraft/recraftv4_1_pro_vector",
        "recraft/recraftv2_vector",
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(schema.capabilities.text_to_image, "{id} is a text-to-image model");
        assert!(
            !schema.params.contains_key("size"),
            "{id} declares a size; Recraft vector models take aspect ratios only"
        );
        assert!(
            matches!(schema.params.get("seed"), Some(ParamSpec::Seed(_))),
            "{id} declares no seed param (random_seed applies to all models)"
        );
        assert!(schema.tags.iter().any(|t| t == "vector"), "{id} is not tagged vector");
    }
}

/// Per-generation rules for the vector models: V4.1 Vector / Pro Vector take
/// 10000-char prompts, no negative_prompt ("V2 / V3 models") and no curated
/// style (styles via style_id); V2 Vector takes 1000-char prompts,
/// negative_prompt, and the V2 Vector curated styles (default `Vector art`).
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("Maximum prompt length")
/// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> (Generate image → Parameters)
/// @see <https://www.recraft.ai/docs/api-reference/styles.md> ("Recraft V2 Vector"), read 2026-09-11
#[test]
fn recraft_vector_models_follow_their_generation_rules() {
    let reg = registry();
    for id in ["recraft/recraftv4_1_vector", "recraft/recraftv4_1_pro_vector"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(schema.prompt.max_length, Some(10000), "{id} prompt max_length");
        assert!(
            !schema.params.contains_key("negative_prompt"),
            "{id} declares negative_prompt, a V2 / V3 parameter"
        );
        assert!(!schema.params.contains_key("style"), "{id} advertises a curated `style`");
        assert!(
            schema.extra_allowlist.iter().any(|k| k == "style_id"),
            "{id} must allowlist style_id, its only style input"
        );
    }
    let id = "recraft/recraftv2_vector";
    let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
    assert_eq!(schema.prompt.max_length, Some(1000), "{id} prompt max_length");
    assert!(schema.params.contains_key("negative_prompt"), "{id} should declare negative_prompt");
    let styles = enum_values(&reg, id, "style");
    for s in ["Vector art", "Icon", "Line art"] {
        assert!(styles.contains(&s.to_string()), "{id} does not offer the {s:?} style");
    }
    match schema.params.get("style") {
        Some(ParamSpec::String(s)) => {
            assert_eq!(s.default.as_deref(), Some("Vector art"), "{id} default style")
        }
        other => panic!("{id} style is {other:?}"),
    }
}

// ─── openai: 2026-09-11 drift decisions applied ─────────────────────────────

/// GPT Image 2.5 Sunburst and Flare (snapshots `*-2026-09-08`): both take
/// images/generations and images/edits (inpainting a listed feature), add the
/// `xhigh` and `max` quality settings on top of GPT Image 2's
/// `low|medium|high|auto` (default `auto`), and share gpt-image-2's custom
/// sizes (both edges multiples of 16, neither above 3840).
/// @see <https://developers.openai.com/api/docs/models/gpt-image-2.5-sunburst> (2026-09-11)
/// @see <https://developers.openai.com/api/docs/models/gpt-image-2.5-flare> (2026-09-11)
/// @see <https://developers.openai.com/api/docs/guides/image-generation> ("Size and quality options", 2026-09-11)
#[test]
fn openai_gpt_image_2_5_models_advertise_xhigh_and_max_quality() {
    let reg = registry();
    for id in [
        "openai/gpt-image-2.5-sunburst",
        "openai/gpt-image-2.5-flare",
    ] {
        let schema = reg
            .get(id)
            .unwrap_or_else(|| panic!("{id} missing from the catalog"));
        assert!(
            schema.capabilities.text_to_image
                && schema.capabilities.image_to_image
                && schema.capabilities.inpainting,
            "{id}: images/generations plus images/edits (with mask inpainting)"
        );
        assert_eq!(
            enum_values(&reg, id, "quality"),
            ["auto", "low", "medium", "high", "xhigh", "max"],
            "{id}: the GPT Image 2.5 quality settings"
        );
        match schema.params.get("size") {
            Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Freeform(f))) => {
                assert_eq!(f.multiple_of, Some(16), "{id}: edges are multiples of 16");
                assert_eq!(
                    (f.max_width, f.max_height),
                    (3840, 3840),
                    "{id}: neither edge may exceed 3840"
                );
            }
            other => panic!("{id} size is {other:?}, expected a freeform WxH size"),
        }
        assert_eq!(
            schema.prompt.max_length,
            Some(32000),
            "{id}: 32000 characters for the GPT image models"
        );
    }
}

// ─── bfl: 2026-09-11 drift decisions applied ────────────────────────────────

/// FLUX.2 [max] (`POST /v1/flux-2-max`) takes the same `Flux2Inputs` body as
/// FLUX.2 [pro]: text-to-image plus `input_image` editing, and `disable_pup`
/// to turn off the prompt upsampling [pro] and [max] apply by default.
/// @see <https://api.bfl.ai/openapi.json> (/v1/flux-2-max, 2026-09-11)
#[test]
fn bfl_flux_2_max_is_advertised_with_the_flux2_request_shape() {
    let id = "bfl/flux-2-max";
    let reg = registry();
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(
        schema.capabilities.text_to_image && schema.capabilities.image_to_image,
        "{id}: generate or edit"
    );
    assert!(
        schema
            .ref_inputs
            .as_ref()
            .is_some_and(|r| r.roles.contains_key("init")),
        "{id}: the edit base is sent as `input_image`"
    );
    assert!(
        schema.extra_allowlist.iter().any(|k| k == "disable_pup"),
        "{id}: Flux2Inputs documents disable_pup"
    );
    assert!(
        matches!(schema.params.get("size"), Some(ParamSpec::Size(_))),
        "{id}: FLUX.2 takes width/height"
    );
}

/// FLUX.2 [klein] 9B (`POST /v1/flux-2-klein-9b`) takes `Flux2KleinInputs`:
/// "like Pro but max 4 images", and without `disable_pup` ([klein] "does not
/// include prompt upsampling").
/// @see <https://api.bfl.ai/openapi.json> (/v1/flux-2-klein-9b, 2026-09-11)
/// @see <https://docs.bfl.ml/flux_2/flux2_overview.md> (2026-09-11)
#[test]
fn bfl_flux_2_klein_9b_is_advertised_without_prompt_upsampling_controls() {
    let id = "bfl/flux-2-klein-9b";
    let reg = registry();
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(
        schema.capabilities.text_to_image && schema.capabilities.image_to_image,
        "{id}: generate or edit"
    );
    assert!(
        !schema.extra_allowlist.iter().any(|k| k == "disable_pup"),
        "{id}: Flux2KleinInputs has no disable_pup"
    );
    let refs = schema
        .ref_inputs
        .as_ref()
        .unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
    assert!(
        refs.max_total <= 4,
        "{id}: [klein] takes at most 4 input images"
    );
}

// ─── stability: 2026-09-11 drift decisions applied ──────────────────────────

/// SD 3.5 Medium runs on `/v2beta/stable-image/generate/sd3` (`model:
/// sd3.5-medium`, a member of that route's `model` enum): text-to-image, or
/// image-to-image with `image` + `strength` (0-1), and the route's
/// `aspect_ratio` enum.
/// @see <https://api.stability.ai/v2alpha/openapi> (2026-09-11)
#[test]
fn stability_sd3_5_medium_is_advertised_on_the_sd3_route() {
    const SD3_RATIOS: [&str; 9] = [
        "21:9", "16:9", "3:2", "5:4", "1:1", "4:5", "2:3", "9:16", "9:21",
    ];
    let id = "stability/sd3.5-medium";
    let reg = registry();
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(
        schema.capabilities.text_to_image && schema.capabilities.image_to_image,
        "{id}: the sd3 route's text-to-image and image-to-image modes"
    );
    let allowed = aspect_ratios(&reg, id);
    assert_eq!(
        allowed.len(),
        SD3_RATIOS.len(),
        "{id}: every sd3 aspect ratio"
    );
    for ar in &allowed {
        assert!(
            SD3_RATIOS.contains(&ar.as_str()),
            "{id} allows {ar:?}, not in the sd3 aspect_ratio enum"
        );
    }
    match schema.params.get("strength") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!((f.min, f.max), (Some(0.0), Some(1.0)), "{id}: strength 0-1")
        }
        other => panic!("{id} strength is {other:?}, expected Float"),
    }
}

// ─── replicate: 2026-09-11 drift decisions applied ──────────────────────────

/// `black-forest-labs/flux-1.1-pro` input: `aspect_ratio` enum `custom | 1:1 |
/// 16:9 | 3:2 | 2:3 | 4:5 | 5:4 | 9:16 | 3:4 | 4:3` (no 21:9 / 9:21; `custom`
/// only unlocks width/height, which the adapter never sends), no guidance or
/// steps input, and its only image input is `image_prompt` (Redux), which the
/// adapter does not send — so text-to-image only.
/// @see <https://replicate.com/black-forest-labs/flux-1.1-pro/api/schema> (2026-09-11)
#[test]
fn replicate_flux_1_1_pro_matches_the_model_input_schema() {
    const FLUX_1_1_PRO_RATIOS: [&str; 9] = [
        "1:1", "16:9", "3:2", "2:3", "4:5", "5:4", "9:16", "3:4", "4:3",
    ];
    let id = "replicate/flux-1.1-pro";
    let reg = registry();
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(schema.capabilities.text_to_image, "{id}: text-to-image");
    assert!(
        !schema.capabilities.image_to_image && schema.ref_inputs.is_none(),
        "{id}: the adapter's `image` input is not in this model's schema"
    );
    let allowed = aspect_ratios(&reg, id);
    assert!(!allowed.is_empty(), "{id} declares no aspect ratios");
    for ar in &allowed {
        assert!(
            FLUX_1_1_PRO_RATIOS.contains(&ar.as_str()),
            "{id} allows {ar:?}, not in flux-1.1-pro's aspect_ratio enum"
        );
    }
    for param in ["guidance_scale", "steps", "negative_prompt"] {
        assert!(
            schema.params.get(param).is_none(),
            "{id} declares `{param}`, which black-forest-labs/flux-1.1-pro does not take"
        );
    }
}

// ─── fal: 2026-09-11 drift decisions applied ────────────────────────────────

/// `fal-ai/flux-2` (FLUX.2 [dev]) input: `guidance_scale` 0-20 (default
/// 2.5), `num_inference_steps` 4-50 (default 28), and `image_size` width and
/// height 512-2048. Text-to-image only: editing is the separate
/// `fal-ai/flux-2/edit` endpoint.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2> (2026-09-11)
#[test]
fn fal_flux_2_matches_the_endpoint_input_schema() {
    let id = "fal/flux-2";
    let reg = registry();
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(
        schema.capabilities.text_to_image && !schema.capabilities.image_to_image,
        "{id}: fal-ai/flux-2 has no image input"
    );
    match schema.params.get("guidance_scale") {
        Some(ParamSpec::Float(f)) => assert_eq!(
            (f.min, f.max, f.default),
            (Some(0.0), Some(20.0), Some(2.5)),
            "{id}: guidance_scale 0-20, default 2.5"
        ),
        other => panic!("{id} guidance_scale is {other:?}, expected Float"),
    }
    match schema.params.get("steps") {
        Some(ParamSpec::Int(i)) => assert_eq!(
            (i.min, i.max, i.default),
            (Some(4), Some(50), Some(28)),
            "{id}: num_inference_steps 4-50, default 28"
        ),
        other => panic!("{id} steps is {other:?}, expected Int"),
    }
    match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Freeform(f))) => {
            assert_eq!(
                (f.min_width, f.max_width, f.min_height, f.max_height),
                (512, 2048, 512, 2048),
                "{id}: width and height between 512 and 2048"
            );
        }
        other => panic!("{id} size is {other:?}, expected a freeform WxH size"),
    }
}

// ─── kling: 2026-09-11 drift decisions applied ──────────────────────────────

/// Kling Image 3.0 on POST /v1/images/generations (`model_name` kling-v3):
/// text + image input, eight aspect ratios including 21:9, 1K/2K via
/// `resolution`, prompt at most 2500 characters. Character / face feature
/// reference (`image_reference`) is "Not Supported" on Image 3.0, and
/// `image_fidelity` / `human_fidelity` are kling-v1/v1-5 only.
/// @see https://kling.ai/document-api/api/image/3-0-omni/image-generation.md (2026-09-11)
/// @see https://kling.ai/document-api/guides/capability-map/image.md (2026-09-11)
#[test]
fn kling_image_3_0_matches_the_image_3_0_contract() {
    let reg = registry();
    let schema = reg.get("kling/kling-v3").expect("kling/kling-v3 missing");
    assert!(schema.capabilities.text_to_image && schema.capabilities.image_to_image);
    assert_eq!(
        schema.prompt.max_length,
        Some(2500),
        "Image 3.0 prompt limit"
    );
    let mut got = aspect_ratios(&reg, "kling/kling-v3");
    got.sort();
    let mut want: Vec<String> = ["16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "21:9"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    want.sort();
    assert_eq!(got, want, "Image 3.0 aspect ratios");
    let refs = schema
        .ref_inputs
        .as_ref()
        .expect("Image 3.0 takes a reference image");
    assert_eq!(refs.max_total, 1, "`image` is a single image");
    assert!(
        schema.extra_allowlist.iter().any(|k| k == "resolution"),
        "resolution selects 1k/2k"
    );
    for key in ["image_reference", "image_fidelity", "human_fidelity"] {
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == key),
            "kling/kling-v3 allowlists {key}, which Image 3.0 does not support"
        );
    }
}

/// Kling 3.0 on the legacy text2video / image2video endpoints (both
/// `model_name` enums list kling-v3): text and image input, 3-15 s,
/// 16:9 | 9:16 | 1:1, mode std | pro | 4k, native audio (`sound`),
/// first + last frame. Camera control is "Not Supported" and cfg_scale is not
/// documented for 3.0, so neither is allowlisted.
/// @see https://kling.ai/document-api/api/video/1-6/text-to-video.md (2026-09-11)
/// @see https://kling.ai/document-api/api/video/2-1/image-to-video.md (2026-09-11)
/// @see https://kling.ai/document-api/guides/capability-map/video.md (2026-09-11)
#[test]
fn kling_v3_video_matches_the_capability_map() {
    let reg = registry();
    let id = "kling/video-kling-v3";
    let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
    assert!(
        schema.capabilities.text_to_video && schema.capabilities.image_to_video,
        "{id}: text and image input"
    );
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(3.0), "{id}: 3~15 s");
            assert_eq!(f.max, Some(15.0), "{id}: 3~15 s");
        }
        other => panic!("{id} duration_seconds is {other:?}"),
    }
    let mut got = aspect_ratios(&reg, id);
    got.sort();
    let mut want: Vec<String> = ["16:9", "9:16", "1:1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    want.sort();
    assert_eq!(got, want, "{id} aspect ratios");
    for key in ["mode", "sound"] {
        assert!(
            schema.extra_allowlist.iter().any(|k| k == key),
            "{id}: Kling 3.0 documents {key}"
        );
    }
    for key in ["cfg_scale", "camera_control"] {
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == key),
            "{id} allowlists {key}, which Kling 3.0 does not support"
        );
    }
    let refs = schema.ref_inputs.as_ref().expect("Kling 3.0 takes frames");
    assert!(refs.roles.contains_key("first_frame"), "{id}: first frame");
    assert!(
        refs.roles.contains_key("last_frame"),
        "{id}: First/Last Frame is Supported"
    );
}

// ─── leonardo: 2026-09-11 drift decisions applied ───────────────────────────

/// Veo 3.1 Fast (`VEO3_1FAST`): the v1 Veo 3.1 guide covers it — "model ...
/// Set to VEO3_1, VEO3_1FAST", "duration ... Set to 4, 6, or 8", "resolution
/// ... Set to RESOLUTION_720 and RESOLUTION_1080"; Leonardo's model schema
/// caps its prompt at 9999 characters.
/// @see <https://docs.leonardo.ai/v1.0/docs/veo-31> (read 2026-09-11)
/// @see <https://docs.leonardo.ai/reference/creategeneration> — Veo3_1FastGenerate001GenerationRequest, prompt maxLength 9999 (read 2026-09-11)
#[test]
fn leonardo_veo3_1_fast_matches_the_veo_3_1_fast_contract() {
    let reg = registry();
    let schema = reg.get("leonardo/veo3.1-fast").expect("veo3.1-fast missing");
    assert!(schema.capabilities.image_to_video);
    assert_eq!(
        enum_values(&reg, "leonardo/veo3.1-fast", "resolution"),
        vec!["720p".to_string(), "1080p".to_string()],
        "Veo 3.1 Fast renders at 720p or 1080p on the v1 API"
    );
    assert_eq!(schema.prompt.max_length, Some(9999), "Veo 3.1 Fast prompt limit");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.min, Some(4.0), "Veo 3.1 Fast: 4, 6 or 8 s");
            assert_eq!(f.max, Some(8.0), "Veo 3.1 Fast: 4, 6 or 8 s");
            assert_eq!(f.default, Some(8.0), "Veo 3.1 Fast defaults to 8 s");
        }
        other => panic!("veo3.1-fast duration_seconds is {other:?}"),
    }
}

// ─── ideogram: 2026-09-11 drift decisions applied ───────────────────────────

/// Ideogram 4.0 is text-to-image only on `POST /v1/ideogram-v4/generate`:
/// the request has `text_prompt`, `resolution`, `rendering_speed` and
/// `enable_copyright_detection` — no aspect_ratio, seed, negative prompt,
/// style or reference-image field. Its `resolution` is the `ResolutionV4`
/// enum ("The 1K and 2K resolutions supported for Ideogram 4.0"), which we
/// advertise as the `size` enum. The three rows mirror the 3.0 speed tiers
/// (FLASH "currently return[s] a 400" on 4.0, so it has no row).
/// @see <https://developer.ideogram.ai/openapi.json> (post_generate_image_v4, ResolutionV4), read 2026-09-11
#[test]
fn ideogram_v4_rows_match_the_v4_generate_contract() {
    use litegen::capabilities::schema::SizeSpec;
    const RESOLUTION_V4: [(u32, u32); 38] = [
        (2048, 2048), (1440, 2880), (2880, 1440), (1664, 2496), (2496, 1664), (1792, 2240),
        (2240, 1792), (1440, 2560), (2560, 1440), (1600, 2560), (2560, 1600), (1728, 2304),
        (2304, 1728), (1296, 3168), (3168, 1296), (1152, 2944), (2944, 1152), (1248, 3328),
        (3328, 1248), (1280, 3072), (3072, 1280), (1024, 3072), (3072, 1024), (1024, 1024),
        (896, 1120), (1120, 896), (864, 1152), (1152, 864), (832, 1248), (1248, 832),
        (800, 1280), (1280, 800), (720, 1280), (1280, 720), (720, 1440), (1440, 720),
        (512, 1536), (1536, 512),
    ];
    let mut theirs = RESOLUTION_V4.to_vec();
    theirs.sort();
    let reg = registry();
    for id in ["ideogram/ideogram-v4", "ideogram/ideogram-v4-turbo", "ideogram/ideogram-v4-quality"] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing from the catalog"));
        assert!(m.capabilities.text_to_image, "{id} must be text-to-image");
        assert!(!m.capabilities.image_to_image, "{id}: v4 generate takes no reference images");
        assert!(m.ref_inputs.is_none(), "{id}: v4 generate takes no reference images");
        for absent in ["aspect_ratio", "seed", "negative_prompt", "style"] {
            assert!(!m.params.contains_key(absent), "{id} advertises {absent}, which v4 generate lacks");
        }
        let mut ours = match m.params.get("size") {
            Some(ParamSpec::Size(SizeSpec::Enum(e))) => e.values.clone(),
            other => panic!("{id} size is {other:?}, expected the ResolutionV4 enum"),
        };
        ours.sort();
        assert_eq!(ours, theirs, "{id} sizes differ from Ideogram's ResolutionV4 enum");
        for key in &m.extra_allowlist {
            assert!(
                ["enable_copyright_detection"].contains(&key.as_str()),
                "{id} allowlists {key:?}, not a v4 generate field"
            );
        }
    }
}

// ─── fal: 2026-09-11 drift decisions applied ────────────────────────────────

/// `fal/ltx-2.3` is text- and image-to-video over fal-ai/ltx-2.3/{text,image}-to-video.
/// Both request schemas: prompt maxLength 5000; duration enum [6, 8, 10]
/// (default 6 — expressed as the 6–10 range); resolution enum [1080p, 1440p,
/// 2160p]; aspect_ratio 16:9 | 9:16 (image-to-video adds `auto`); fps enum
/// [24, 25, 48, 50]; no seed.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-2.3/text-to-video>, read 2026-09-11
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/ltx-2.3/image-to-video>, read 2026-09-11
#[test]
fn fal_ltx_2_3_matches_the_ltx_2_3_endpoint_schemas() {
    let reg = registry();
    let id = "fal/ltx-2.3";
    let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(m.capabilities.text_to_video && m.capabilities.image_to_video, "{id} capabilities");
    assert_eq!(m.prompt.max_length, Some(5000), "{id} prompt maxLength is 5000");
    match m.params.get("duration_seconds") {
        Some(ParamSpec::Float(d)) => {
            assert_eq!((d.min, d.max, d.default), (Some(6.0), Some(10.0), Some(6.0)), "{id} duration is 6 | 8 | 10, default 6");
        }
        other => panic!("{id} duration_seconds is {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "resolution"), ["1080p", "1440p", "2160p"], "{id} resolution enum");
    let mut ars = aspect_ratios(&reg, id);
    ars.sort();
    assert_eq!(ars, ["16:9", "9:16"], "{id} aspect ratios (both endpoints)");
    match m.params.get("fps") {
        Some(ParamSpec::Int(f)) => assert_eq!((f.min, f.max), (Some(24), Some(50)), "{id} fps is 24 | 25 | 48 | 50"),
        other => panic!("{id} fps is {other:?}"),
    }
    assert!(!m.params.contains_key("seed"), "{id}: neither LTX-2.3 endpoint takes a seed");
    let ri = m.ref_inputs.as_ref().expect("start frame ref input");
    assert_eq!(ri.max_total, 1);
    let init = ri.roles.get("init").expect("init role");
    assert!(!init.required, "{id} is text-to-video without a start frame");
}

// ─── minimax: 2026-09-11 drift decisions applied ────────────────────────────

/// MiniMax-H3 does text-to-video, first/last-frame image-to-video and
/// reference-to-video; `768P` / `2K`; integer 4–15 s; prompt ≤ 7000 characters.
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-09-11)
#[test]
fn minimax_h3_matches_the_v2_create_contract() {
    let reg = registry();
    let schema = reg.get("minimax/MiniMax-H3").expect("minimax/MiniMax-H3 missing");
    assert!(schema.capabilities.text_to_video && schema.capabilities.image_to_video);
    assert_eq!(enum_values(&reg, "minimax/MiniMax-H3", "resolution"), vec!["768P", "2K"]);
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max), (Some(4), Some(15))),
        other => panic!("MiniMax-H3 duration_seconds is {other:?}, expected an integer 4-15"),
    }
    assert_eq!(schema.prompt.max_length, Some(7000));
    let roles = &schema.ref_inputs.as_ref().expect("MiniMax-H3 takes image inputs").roles;
    assert_eq!(roles.get("first_frame").map(|r| r.max_count), Some(1));
    assert_eq!(roles.get("last_frame").map(|r| r.max_count), Some(1));
    assert_eq!(roles.get("reference").map(|r| r.max_count), Some(9), "reference images ≤ 9");
    // Text-to-video rejects `adaptive`, and one list serves every mode.
    let ratios = aspect_ratios(&reg, "minimax/MiniMax-H3");
    assert!(!ratios.is_empty() && !ratios.iter().any(|r| r == "adaptive"), "{ratios:?}");
}

/// MiniMax-H3-Max: text-to-video and first/last-frame image-to-video only (no
/// reference-to-video); `480P` / `768P` ("2K is not supported"); 5–15 s ("4
/// seconds is not supported").
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-09-11)
#[test]
fn minimax_h3_max_has_no_2k_no_4s_and_no_reference_role() {
    let reg = registry();
    let schema = reg.get("minimax/MiniMax-H3-Max").expect("minimax/MiniMax-H3-Max missing");
    assert!(schema.capabilities.text_to_video && schema.capabilities.image_to_video);
    assert_eq!(enum_values(&reg, "minimax/MiniMax-H3-Max", "resolution"), vec!["480P", "768P"]);
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max), (Some(5), Some(15))),
        other => panic!("MiniMax-H3-Max duration_seconds is {other:?}, expected an integer 5-15"),
    }
    let roles = &schema.ref_inputs.as_ref().expect("MiniMax-H3-Max takes first/last frames").roles;
    assert!(roles.contains_key("first_frame") && roles.contains_key("last_frame"));
    assert!(!roles.contains_key("reference"), "H3 Max has no reference-to-video");
}

// ─── hunyuan: 2026-09-11 drift decisions applied ────────────────────────────

/// Hunyuan Image 3.0 is aiart `SubmitTextToImageJob` ("提交混元生图（3.0）任务"):
/// Prompt is required and "最多可传8192个 utf-8 字符"; Images is "参考图，最多三张图".
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/aiart/v20221229/models.go> (2026-09-11)
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-cli/master/tccli/services/aiart/v20221229/api.json> (2026-09-11)
#[test]
fn hunyuan_image_3_is_text_and_image_to_image_with_up_to_three_references() {
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-image-3").expect("hunyuan-image-3 missing");
    assert!(schema.capabilities.text_to_image, "SubmitTextToImageJob takes a prompt alone");
    assert!(schema.capabilities.image_to_image, "SubmitTextToImageJob takes reference Images");
    assert!(schema.prompt.required, "Prompt is required");
    assert_eq!(schema.prompt.max_length, Some(8192), "Prompt is at most 8192 characters");
    let ri = schema.ref_inputs.as_ref().expect("hunyuan-image-3 declares no ref_inputs");
    assert_eq!(ri.max_total, 3, "Images: at most three reference images");
    let init = ri.roles.get("init").expect("hunyuan-image-3 declares no init role");
    assert!(!init.required && init.min_count == 0, "Images is optional");
    assert_eq!(init.max_count, 3);
}

/// `SubmitTextToImageJob.Seed`: "取值范围：1 - 4294967295". `Resolution`: width and
/// height in [512, 2048], W*H at most 1024*1024, output snapped to the
/// documented size list (37 entries, default 1024:1024).
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/aiart/v20221229/models.go> (2026-09-11)
#[test]
fn hunyuan_image_3_seed_and_sizes_match_submit_text_to_image_job() {
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-image-3").expect("hunyuan-image-3 missing");
    match schema.params.get("seed") {
        Some(ParamSpec::Seed(s)) => {
            assert_eq!((s.min, s.max), (1, 4_294_967_295), "Seed range is 1 - 4294967295");
        }
        other => panic!("hunyuan-image-3 seed is {other:?}, expected Seed"),
    }
    let sizes = match schema.params.get("size") {
        Some(ParamSpec::Size(litegen::capabilities::schema::SizeSpec::Enum(e))) => e.values.clone(),
        other => panic!("hunyuan-image-3 size is {other:?}, expected the documented size list"),
    };
    assert_eq!(sizes.len(), 37, "the documented size list has 37 entries");
    for &(w, h) in &sizes {
        assert!((512..=2048).contains(&w) && (512..=2048).contains(&h), "{w}x{h} outside [512, 2048]");
        assert!(w * h <= 1024 * 1024, "{w}x{h} exceeds 1024*1024 pixels");
    }
    for s in [(1024, 1024), (2048, 512), (512, 2048), (1280, 720), (720, 1280), (768, 1024)] {
        assert!(sizes.contains(&s), "{s:?} is in the documented size list");
    }
    assert!(!schema.params.contains_key("negative_prompt"), "SubmitTextToImageJob has no NegativePrompt");
}

/// The adapter merges every allowlisted extra key into the request verbatim,
/// and Tencent fails an undefined parameter (UnknownParameter).
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/aiart/v20221229/models.go> (2026-09-11)
#[test]
fn hunyuan_image_3_extra_allowlist_names_only_submit_text_to_image_job_parameters() {
    const PARAMS: [&str; 7] = ["Prompt", "Images", "Resolution", "Seed", "LogoAdd", "LogoParam", "Revise"];
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-image-3").expect("hunyuan-image-3 missing");
    for key in &schema.extra_allowlist {
        assert!(
            PARAMS.contains(&key.as_str()),
            "hunyuan-image-3 allowlists {key:?}, which is not a SubmitTextToImageJob parameter"
        );
    }
}

/// `SubmitImageToVideoGeneralJob` (图生视频通用能力): Image is its only required
/// member, Prompt is optional and "最多支持200个 utf-8 字符".
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/models.go> (2026-09-11)
/// @see <https://cloud.tencent.com/document/api/1616/124465> (2026-09-11)
#[test]
fn hunyuan_video_i2v_requires_one_image_and_an_optional_short_prompt() {
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-video-i2v").expect("hunyuan-video-i2v missing");
    assert!(schema.capabilities.image_to_video);
    assert!(!schema.capabilities.text_to_video, "Image is required: no text-only jobs");
    assert!(!schema.prompt.required, "Prompt is optional");
    assert_eq!(schema.prompt.max_length, Some(200), "Prompt is at most 200 characters");
    let ri = schema.ref_inputs.as_ref().expect("hunyuan-video-i2v declares no ref_inputs");
    assert_eq!(ri.max_total, 1);
    let first = ri.roles.get("first_frame").expect("hunyuan-video-i2v declares no first_frame role");
    assert!(first.required && first.min_count == 1 && first.max_count == 1, "exactly one Image");
}

/// `Resolution`: "可选择：480p、720p、1080p". `Fps`: "从16, 24, 30中选择。默认值：30".
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/models.go> (2026-09-11)
#[test]
fn hunyuan_video_i2v_resolution_and_fps_match_submit_image_to_video_general_job() {
    let reg = registry();
    assert_eq!(
        enum_values(&reg, "hunyuan/hunyuan-video-i2v", "resolution"),
        vec!["480p".to_string(), "720p".to_string(), "1080p".to_string()],
        "SubmitImageToVideoGeneralJob renders 480p, 720p or 1080p"
    );
    let schema = reg.get("hunyuan/hunyuan-video-i2v").unwrap();
    match schema.params.get("fps") {
        Some(ParamSpec::Int(f)) => {
            assert_eq!((f.min, f.max, f.default), (Some(16), Some(30), Some(30)), "Fps is 16, 24 or 30 (default 30)");
        }
        other => panic!("hunyuan-video-i2v fps is {other:?}, expected Int"),
    }
}

/// The adapter merges every allowlisted extra key into the request verbatim,
/// and Tencent fails an undefined parameter (UnknownParameter).
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/vclm/v20240523/models.go> (2026-09-11)
#[test]
fn hunyuan_video_i2v_extra_allowlist_names_only_submit_image_to_video_general_job_parameters() {
    const PARAMS: [&str; 6] = ["Image", "Prompt", "Resolution", "Fps", "LogoAdd", "LogoParam"];
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-video-i2v").expect("hunyuan-video-i2v missing");
    for key in &schema.extra_allowlist {
        assert!(
            PARAMS.contains(&key.as_str()),
            "hunyuan-video-i2v allowlists {key:?}, which is not a SubmitImageToVideoGeneralJob parameter"
        );
    }
}
