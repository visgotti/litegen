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
        ("google/veo-3.0-generate-001", "Google shutdown 2026-06-30"),
        ("google/veo-3.0-fast-generate-001", "Google shutdown 2026-06-30"),
        ("runway/gen-3", "gen3a_turbo removed from the API 2026-07-30"),
        ("runway/gen-3-turbo", "gen3a_turbo removed from the API 2026-07-30"),
        ("bfl/flux-pro", "no /v1/flux-pro endpoint in api.bfl.ai/openapi.json"),
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

/// The Hunyuan video adapter only ever calls `SubmitImageToVideoJob`, which
/// needs a driving image. vclm does expose `SubmitTextToVideoJob`, but we do
/// not implement it — so the catalog must not promise text-to-video.
#[test]
fn hunyuan_video_advertises_only_what_the_adapter_implements() {
    let reg = registry();
    let schema = reg.get("hunyuan/hunyuan-video").expect("hunyuan-video missing");
    assert!(
        !schema.capabilities.text_to_video,
        "the adapter has no SubmitTextToVideoJob path"
    );
    assert!(schema.capabilities.image_to_video);
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
