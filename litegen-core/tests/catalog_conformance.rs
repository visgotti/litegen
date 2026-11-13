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

/// The weekly drift check (`scripts/model-drift/`) watches only the models its
/// provider definition maps. Its node test enforces that, but a change verified
/// with `cargo test` alone never runs it — which is how three 3D models reached
/// the catalog unwatched. Each id must be a quoted key in
/// `scripts/model-drift/providers/<provider>.mjs`.
#[test]
fn every_model_is_mapped_in_its_drift_definition() {
    let mut root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    let mut unmapped: Vec<String> = registry()
        .all()
        .filter(|m| m.provider != "mock")
        .filter(|m| {
            let path = root.join(format!("scripts/model-drift/providers/{}.mjs", m.provider));
            let src = std::fs::read_to_string(path).unwrap_or_default();
            !src.contains(&format!("'{}'", m.id)) && !src.contains(&format!("\"{}\"", m.id))
        })
        .map(|m| format!("{} → scripts/model-drift/providers/{}.mjs", m.id, m.provider))
        .collect();
    unmapped.sort();
    assert!(
        unmapped.is_empty(),
        "map each to the vendor id(s) its adapter sends, then run \
         `node --test 'scripts/model-drift/test/*.test.mjs'`: {unmapped:#?}"
    );
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
        // "Kling 2.1 Pro is being retired on Tuesday, September 15, 2026 at
        // 9:30 AM AEST (UTC+10)." https://docs.leonardo.ai/docs/kling-2-1-pro.md
        ("leonardo/kling2.1", "Leonardo retired Kling 2.1 Pro (KLING2_1) 2026-09-15; successor Kling 3.0 Pro is v2-API only"),
        ("hunyuan/hunyuan-image-3", "Tencent deleted aiart SubmitTextToImageJob and QueryTextToImageJob 2026-09-22; successor TokenHub hy-image-v3"),
        ("luma/ray-3", "not in the Dream Machine model enum (ray-2 | ray-flash-2); ray-3.2 is Luma Agents API only"),
        ("luma/ray-hdr-3", "not in the Dream Machine model enum (ray-2 | ray-flash-2); ray-3.2 is Luma Agents API only"),
        ("bytedance/seedream-3-0-t2i-250415", "BytePlus deactivated it 2026-05-13; replacement bytedance/seedream-5-0-lite-260128"),
        ("bytedance/doubao-seedance-1-0-lite-i2v-250428", "BytePlus deactivated it 2026-05-13; replacement bytedance/seedance-1-0-pro-fast-251015"),
        // Scheduled shutdowns that took effect (2026-10-12 model-drift run):
        // pages and enum entries gone from https://kling.ai/document-api/api/image/2-1/image-generation.md
        ("kling/kling-v2", "Kling retired Image 2.0 2026-09-15; successor kling/kling-v2-1"),
        ("kling/kling-v1-5", "Kling retired Image 1.5 2026-09-15; successor kling/kling-v2-1"),
        ("kling/video-kling-v1-6", "Kling retired Video 1.6 2026-09-15; successors video-kling-v2-5-turbo / -v2-6"),
        ("kling/video-kling-v2-1", "Kling retired Video 2.1 2026-09-15; successors video-kling-v2-5-turbo / -v2-6"),
        // https://developers.openai.com/api/docs/deprecations: "Past deprecations"
        ("openai/sora", "OpenAI shut down the Videos API and Sora 2 2026-09-24; no replacement"),
        ("openai/sora-2-pro", "OpenAI shut down the Videos API and Sora 2 2026-09-24; no replacement"),
        // https://docs.aws.amazon.com/bedrock/latest/userguide/model-lifecycle-legacy.md: "Models that have reached EOL"
        ("bedrock/amazon.nova-canvas-v1:0", "Bedrock EOL 2026-09-30; no Amazon successor"),
        ("bedrock/amazon.nova-reel-v1:1", "Bedrock EOL 2026-09-30; no Amazon successor"),
        // Scheduled shutdowns that took effect (2026-10-30 build): OpenAI's
        // deprecations page, and Google's for the Gemini API's Veo previews.
        // @see https://developers.openai.com/api/docs/deprecations
        // @see https://ai.google.dev/gemini-api/docs/deprecations
        ("openai/gpt-image-1", "OpenAI shut down gpt-image-1 2026-10-23; replacement openai/gpt-image-2.5-sunburst or -flare"),
        ("google/veo-3.1-generate-preview", "Google shut down the Veo 3.1 previews 2026-10-22; successor gemini-omni-1.1-flash (Interactions API, not carried)"),
        ("google/veo-3.1-fast-generate-preview", "Google shut down the Veo 3.1 previews 2026-10-22; successor gemini-omni-1.1-flash (Interactions API, not carried)"),
        ("google/veo-3.1-lite-generate-preview", "Google shut down the Veo 3.1 previews 2026-10-22; successor gemini-omni-1.1-flash (Interactions API, not carried)"),
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
    // @see https://ai.google.dev/gemini-api/docs/deprecations
    // Google moved this shutdown from 2026-10-02 to 2027-03-15 (deprecations page, 2026-10-12).
    ("google/gemini-2.5-flash-image", "2027-03-15", "Google shuts down gemini-2.5-flash-image; successor google/gemini-3.1-flash-lite-image"),
];

/// L14: a scheduled shutdown is the row's own `retires_at`, so litegen stops
/// serving the model the day after it by itself, and the listing says when.
#[test]
fn every_scheduled_shutdown_is_its_rows_retires_at() {
    let reg = registry();
    for (id, date, _) in SCHEDULED_SHUTDOWNS {
        let row = reg.get(id).unwrap_or_else(|| panic!("{id} is scheduled but not in the catalog"));
        assert_eq!(row.retires_at.as_deref(), Some(*date), "{id}: retires_at");
    }
    for m in reg.all().filter(|m| m.retires_at.is_some()) {
        assert!(SCHEDULED_SHUTDOWNS.iter().any(|(id, ..)| *id == m.id), "{} retires but is not in SCHEDULED_SHUTDOWNS", m.id);
    }
}

/// A `replaced_by` names a model the catalog serves.
#[test]
fn every_replacement_is_a_catalog_model() {
    let reg = registry();
    for m in reg.all() {
        if let Some(to) = &m.replaced_by {
            let next = reg.get(to).unwrap_or_else(|| panic!("{} is replaced_by {to}, which the catalog does not carry", m.id));
            assert!(next.retires_at.is_none(), "{} is replaced by {to}, which itself retires", m.id);
        }
    }
}

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
        "openai/gpt-image-2.5-sunburst",
        "openai/gpt-image-2.5-flare",
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
    for id in ["openai/gpt-image-2"] {
        for q in enum_values(&reg, id, "quality") {
            assert!(
                ["auto", "low", "medium", "high"].contains(&q.as_str()),
                "{id} allows quality {q:?}; only auto/low/medium/high apply to GPT image models"
            );
        }
    }
}

/// MiniMax-H3-Highspeed and MiniMax-H3-Lite on the V2 video API (new
/// 2026-11-03): each "Supports text-to-video, image-to-video (first / last
/// frame), and reference-to-video …; `480P` / `768P` resolution, **`2K` is
/// not supported**; 4–15s duration"; "reference images ≤ 9". Prices per
/// second: Highspeed "768P $0.06", "480P $0.04"; Lite "768P $0.04", "480P
/// $0.025" — the default request (768P, 5 s) is $0.30 and $0.20.
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-11-03)
/// @see <https://platform.minimax.io/docs/pricing/overview.md> (2026-11-03)
#[test]
fn minimax_h3_highspeed_and_lite_are_carried_with_their_documented_contract() {
    let reg = registry();
    for (id, price) in [("minimax/MiniMax-H3-Highspeed", 0.30), ("minimax/MiniMax-H3-Lite", 0.20)] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(schema.capabilities.text_to_video && schema.capabilities.image_to_video, "{id}");
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(4), Some(15), Some(5)), "{id}"),
            other => panic!("{id} duration_seconds is {other:?}"),
        }
        let mut res = enum_values(&reg, id, "resolution");
        res.sort();
        assert_eq!(res, ["480P", "768P"], "{id}: no 2K");
        let refs = schema.ref_inputs.as_ref().unwrap();
        assert_eq!(refs.roles.get("reference").map(|r| r.max_count), Some(9), "{id}");
        assert!(refs.roles.contains_key("first_frame") && refs.roles.contains_key("last_frame"), "{id}");
        assert!((schema.pricing.base_cost_usd - price).abs() < 1e-9, "{id}");
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

/// MiniMax `image-01` (`POST /v1/image_generation`): "width … Only effective
/// for `image-01`. Must be set together with `height`. Range [512, 2048],
/// must be divisible by 8" is the canonical `size`, and "seed … Using the same
/// seed and parameters produces reproducible images" the canonical `seed`. The
/// row declared neither.
/// @see <https://platform.minimax.io/docs/api-reference/image-generation-t2i.md> (2026-11-03)
#[test]
fn minimax_image_01_takes_a_size_and_a_seed() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let schema = reg.get("minimax/image-01").expect("image-01 missing");
    match schema.params.get("size") {
        Some(ParamSpec::Size(SizeSpec::Freeform(f))) => {
            assert_eq!((f.min_width, f.max_width, f.min_height, f.max_height), (512, 2048, 512, 2048), "[512, 2048]");
            assert_eq!(f.multiple_of, Some(8), "divisible by 8");
        }
        other => panic!("image-01 size is {other:?}, expected a freeform 512-2048 box"),
    }
    assert!(matches!(schema.params.get("seed"), Some(ParamSpec::Seed(_))), "image-01 documents a reproducible seed");
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

/// Every Vidu endpoint our rows reach takes `seed`: "Random seed When not
/// passed or passed as 0, a random number will be used instead Manual setting
/// uses the specified seed" (text2video, img2video, start-end2video and
/// reference2video alike). No row declared it; 0 means random, so a seed
/// starts at 1.
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/start-end-to-video.md> (vidu2.0, 2026-11-03)
#[test]
fn every_vidu_row_takes_a_seed_that_fixes_the_clip() {
    let reg = registry();
    let rows: Vec<_> = reg.served().into_iter().filter(|m| m.provider == "vidu").collect();
    assert!(!rows.is_empty());
    for m in rows {
        match m.params.get("seed") {
            Some(ParamSpec::Seed(s)) => assert_eq!((s.min, s.max), (1, 2147483647), "{}: 0 is Vidu's random", m.id),
            other => panic!("{} seed is {other:?}", m.id),
        }
    }
}

/// Vidu's `audio` switch ("Whether to use the direct audio-video output
/// capability") is on viduq1's text2video ("Default: true") and
/// reference2video ("Default: false") and on viduq2-pro's img2video
/// ("Default: false"); neither row declared it. Its other endpoints have no
/// switch, which the adapter enforces (see its tests).
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/image-to-video.md> (2026-11-03)
#[test]
fn vidu_q1_and_q2_pro_offer_their_audio_switch() {
    let reg = registry();
    for id in ["vidu/viduq1", "vidu/viduq2-pro"] {
        assert!(matches!(reg.get(id).unwrap().params.get("audio"), Some(ParamSpec::Bool(_))), "{id} documents `audio`");
    }
}

/// vidu2.0: "Duration | 4s, 8s" (Model Map), and the price table lists no
/// other length ("Image-to-video | vidu2.0 | 4S | …", "… | 8S | 720p | …").
/// The row took any whole second from 4 to 8, and the adapter snapped 5–7 to
/// 4 or 8 after billing the length asked for.
/// @see <https://platform.vidu.com/docs/model-map.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/pricing.md> (2026-11-03)
#[test]
fn vidu2_0_takes_4_or_8_seconds() {
    let reg = registry();
    match reg.get("vidu/vidu2.0").expect("vidu2.0 missing").params.get("duration_seconds") {
        Some(ParamSpec::Int(d)) => assert_eq!((d.values.clone(), d.default), (vec![4, 8], Some(4)), "4 or 8 s"),
        other => panic!("vidu2.0 duration_seconds is {other:?}"),
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
        Some(ParamSpec::Int(f)) => {
            assert_eq!(f.min, Some(5), "viduq1 is 5s only");
            assert_eq!(f.max, Some(5), "viduq1 is 5s only");
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
    // An operation (`rig`, `animate`, …) declares its task and its
    // source_model instead: it has no generation mode by design.
    for m in models.into_iter().filter(|m| m.task == litegen::capabilities::ModelTask::Generate) {
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
    // controls in the Playground and in any schema-driven client panel. The
    // one exception is `fps` on a model that makes clips (3D doc §3C: "fps
    // reuses the existing canonical param"): clips have a frame rate.
    let reg = registry();
    for m in model3d_models(&reg) {
        for forbidden in ["size", "fps", "duration_seconds", "aspect_ratio"] {
            if forbidden == "fps" && m.params.contains_key("animations") {
                continue;
            }
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
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-10-30)
#[test]
fn runway_gen4_turbo_is_image_to_video_only() {
    let reg = registry();
    let schema = reg.get("runway/gen4-turbo").expect("runway/gen4-turbo missing");
    assert!(
        !schema.capabilities.text_to_video,
        "gen4_turbo is not accepted by /v1/text_to_video"
    );
    assert!(schema.capabilities.image_to_video);
    // promptImage is required: the start frame arrives as first_frame (or
    // init, the role this row took before first_frame), and a prompt-only
    // request is refused for want of one, before anything is billed.
    let roles = &schema.ref_inputs.as_ref().expect("gen4-turbo takes a start frame").roles;
    assert!(roles.contains_key("first_frame") && roles.contains_key("init"), "{:?}", roles.keys());
    let prompt_only: litegen::types::VideoGenerationRequest =
        serde_json::from_value(serde_json::json!({ "model": "runway/gen4-turbo", "prompt": "a drone shot" })).unwrap();
    let err = litegen::api::middleware::validator::validate_video(schema, prompt_only).unwrap_err();
    assert_eq!(err.code, "capability_unsupported", "{}", err.message);
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
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-10-30) — `referenceImages`: "An array of
///   one to three images to be used as references for the generated image output."
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

/// The image models Runway resells on `POST /v1/text_to_image`, carried as
/// runway routes beside each vendor's own (the hub-route policy, 2026-11-04).
/// Each row matches its model's branch:
/// - the `promptText` maxLength;
/// - the aspect ratios and resolution tiers its `ratio` enum covers, every
///   aspect at every tier (image/runway.rs maps each cell to its size);
/// - its `referenceImages` maxItems;
/// - the price of its default request, at "$0.01 per credit".
///
/// recraft_v4_1_flash and recraft_v4_1_vector have no published price, so they
/// are not carried.
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
/// @see <https://docs.dev.runwayml.com/guides/pricing.md> (2026-11-10)
#[test]
fn runway_resold_image_rows_are_carried_with_runways_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    use litegen::capabilities::schema::MaskConvention;
    struct Row {
        id: &'static str,
        prompt_max: usize,
        aspects: &'static [&'static str],
        resolutions: &'static [&'static str],
        max_refs: u32,
        base_usd: f64,
    }
    let reg = registry();
    for r in [
        Row { id: "runway/gpt_image_2", prompt_max: 32000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &["1K", "2K", "4K"], max_refs: 16, base_usd: 0.2 },
        Row { id: "runway/gpt_image_2_5_flare", prompt_max: 32000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &["1K", "2K", "4K"], max_refs: 16, base_usd: 0.11 },
        Row { id: "runway/gpt_image_2_5_sunburst", prompt_max: 32000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &["1K", "2K", "4K"], max_refs: 16, base_usd: 0.11 },
        Row { id: "runway/gpt_image_2_6_flare", prompt_max: 32000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &["1K", "2K", "4K"], max_refs: 16, base_usd: 0.11 },
        Row { id: "runway/gpt_image_2_6_sunburst", prompt_max: 32000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &["1K", "2K", "4K"], max_refs: 16, base_usd: 0.11 },
        Row { id: "runway/gemini_2.5_flash", prompt_max: 45000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &[], max_refs: 3, base_usd: 0.05 },
        Row { id: "runway/gemini_image3_pro", prompt_max: 45000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &["1K", "2K", "4K"], max_refs: 14, base_usd: 0.2 },
        Row { id: "runway/gemini_image3.1_flash", prompt_max: 45000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9", "1:4", "4:1", "1:8", "8:1"], resolutions: &["0.5K", "1K", "2K", "4K"], max_refs: 14, base_usd: 0.07 },
        Row { id: "runway/gemini_image3.1_flash_lite", prompt_max: 45000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], resolutions: &[], max_refs: 14, base_usd: 0.04 },
        Row { id: "runway/nano_banana_2.1", prompt_max: 45000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9", "1:4", "4:1", "1:8", "8:1"], resolutions: &["1K", "2K", "4K"], max_refs: 14, base_usd: 0.04 },
        Row { id: "runway/muse_image", prompt_max: 4000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "21:9"], resolutions: &[], max_refs: 10, base_usd: 0.01 },
        Row { id: "runway/seedream5_pro", prompt_max: 4000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9"], resolutions: &["1K", "2K"], max_refs: 10, base_usd: 0.05 },
        Row { id: "runway/seedream5_lite", prompt_max: 4000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "21:9"], resolutions: &["2K", "3K"], max_refs: 14, base_usd: 0.04 },
        Row { id: "runway/seedream5_flash", prompt_max: 4000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "21:9"], resolutions: &["1K", "2K"], max_refs: 10, base_usd: 0.02 },
        Row { id: "runway/grok_imagine_image_2", prompt_max: 2500, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "2:1", "1:2", "19.5:9", "9:19.5", "20:9", "9:20"], resolutions: &["1K", "2K"], max_refs: 3, base_usd: 0.06 },
        Row { id: "runway/bria_fibo_1_5", prompt_max: 10000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9"], resolutions: &["1K", "2K"], max_refs: 1, base_usd: 0.05 },
        Row { id: "runway/bria_fibo_edit_1_5", prompt_max: 10000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9"], resolutions: &[], max_refs: 4, base_usd: 0.05 },
        Row { id: "runway/ideogram_4_0", prompt_max: 4000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "12:5", "5:12"], resolutions: &[], max_refs: 1, base_usd: 0.07 },
        Row { id: "runway/ideogram_4_5", prompt_max: 10000, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "2:1", "1:2"], resolutions: &["1K", "2K"], max_refs: 5, base_usd: 0.25 },
        Row { id: "runway/ideogram_vector", prompt_max: 9500, aspects: &[], resolutions: &[], max_refs: 5, base_usd: 0.17 },
    ] {
        let id = r.id;
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(m.provider == "runway" && m.media_type == litegen::capabilities::MediaType::Image, "{id}");
        assert_eq!(m.prompt.max_length, Some(r.prompt_max), "{id}: promptText maxLength");
        assert_eq!(m.docs_url.as_deref(), Some("https://docs.dev.runwayml.com/api/#tag/Start-generating/paths/~1v1~1text_to_image/post"), "{id}");
        assert_eq!(aspect_ratios(&reg, id), r.aspects, "{id}: the aspects its ratio enum covers");
        assert_eq!(enum_values(&reg, id, "resolution"), r.resolutions, "{id}: its ratio enum's tiers");
        let init = m.ref_inputs.as_ref().and_then(|ri| ri.roles.get("init")).map(|role| role.max_count);
        assert_eq!(init, Some(r.max_refs), "{id}: referenceImages maxItems");
        assert!((m.pricing.base_cost_usd - r.base_usd).abs() < 1e-9, "{id}: {} (want {})", m.pricing.base_cost_usd, r.base_usd);
        // One image per call: outputCount is litegen's, never a caller's.
        assert!(!m.extra_allowlist.iter().any(|k| k == "outputCount" || k == "ratio" || k == "model"), "{id}: {:?}", m.extra_allowlist);
    }

    // Prices past the default, read from the guide's tables.
    let price = |id: &str, params: serde_json::Value, refs: usize| {
        let m = reg.get(id).unwrap();
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        req.as_object_mut().unwrap().extend(params.as_object().unwrap().clone());
        req["reference_images"] = (0..refs).map(|i| serde_json::json!({ "type": "url", "value": format!("https://example.com/{i}.png") })).collect();
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    use serde_json::json as j;
    for (id, params, refs, usd) in [
        // gpt_image_2: "| `medium` | 5 | 11 |" (1K / 2K, 4K); "| `low` | 1 | 2 |".
        ("runway/gpt_image_2", j!({ "quality": "medium", "resolution": "2K" }), 0, 0.05),
        ("runway/gpt_image_2", j!({ "quality": "low", "resolution": "4K" }), 3, 0.02),
        // 2.5: 4K `max` 1:1 is 75, 2K `xhigh` 21:9 is 8, 1K `low` 4:5 is 1;
        // "(credits per image + 1 credit per reference image)".
        ("runway/gpt_image_2_5_flare", j!({ "quality": "max", "resolution": "4K" }), 2, 0.77),
        ("runway/gpt_image_2_5_flare", j!({ "quality": "low", "resolution": "4K", "aspect_ratio": "2:3" }), 0, 0.02),
        ("runway/gpt_image_2_6_sunburst", j!({ "quality": "medium", "resolution": "4K", "aspect_ratio": "21:9" }), 0, 0.02),
        ("runway/gpt_image_2_5_sunburst", j!({ "quality": "xhigh", "resolution": "2K", "aspect_ratio": "21:9" }), 0, 0.08),
        ("runway/gpt_image_2_6_flare", j!({ "quality": "low", "aspect_ratio": "4:5" }), 1, 0.02),
        ("runway/gpt_image_2_6_sunburst", j!({ "resolution": "2K", "aspect_ratio": "16:9" }), 0, 0.06),
        // "5 credits per 512px image, … 16 credits per 4K image".
        ("runway/gemini_image3.1_flash", j!({ "resolution": "0.5K" }), 0, 0.05),
        ("runway/gemini_image3.1_flash", j!({ "resolution": "4K" }), 0, 0.16),
        ("runway/gemini_image3_pro", j!({ "resolution": "4K" }), 0, 0.4),
        ("runway/nano_banana_2.1", j!({ "resolution": "2K" }), 0, 0.06),
        ("runway/seedream5_pro", j!({ "resolution": "2K" }), 0, 0.09),
        // Grok: "four 1K `medium` images with two references is `4 × 6 + 2`".
        ("runway/grok_imagine_image_2", j!({ "quality": "low", "resolution": "2K" }), 3, 0.09),
        ("runway/grok_imagine_image_2", j!({}), 2, 0.08),
        ("runway/bria_fibo_1_5", j!({ "resolution": "2K" }), 0, 0.1),
        ("runway/ideogram_4_0", j!({ "quality": "high" }), 0, 0.11),
        // Ideogram 4.5 "defaults to `medium` when generating from reference
        // images and `high` otherwise, including `preciseEdit`".
        ("runway/ideogram_4_5", j!({}), 1, 0.08),
        ("runway/ideogram_4_5", j!({ "extra": { "preciseEdit": true } }), 1, 0.25),
        ("runway/ideogram_4_5", j!({ "quality": "very_low" }), 1, 0.01),
        ("runway/ideogram_vector", j!({ "quality": "high" }), 0, 0.22),
        ("runway/ideogram_vector", j!({ "quality": "high" }), 1, 0.35),
    ] {
        let got = price(id, params.clone(), refs);
        assert!((got - usd).abs() < 1e-9, "{id} {params} + {refs} refs: {got} (want {usd})");
    }

    // Bria FIBO Edit "1 to 4 images to edit": no prompt-only request.
    let edit = reg.get("runway/bria_fibo_edit_1_5").unwrap();
    let init = &edit.ref_inputs.as_ref().unwrap().roles["init"];
    assert!(init.required && init.min_count == 1 && !edit.capabilities.text_to_image);
    // Ideogram 4.5 `maskUri`: "Black areas are edited and white areas are preserved".
    let i45 = reg.get("runway/ideogram_4_5").unwrap();
    assert_eq!(i45.ref_inputs.as_ref().unwrap().roles["mask"].convention, Some(MaskConvention::BlackEdits));
    assert!(i45.capabilities.inpainting && i45.params.contains_key("region"));
    // `quality` that changes Runway's own default by request is left unset.
    assert!(matches!(i45.params.get("quality"), Some(ParamSpec::String(s)) if s.default.is_none()));
    // "recraft_v4_1_flash" and "recraft_v4_1_vector": no price on the guide.
    for id in ["runway/recraft_v4_1_flash", "runway/recraft_v4_1_vector"] {
        assert!(reg.get(id).is_none(), "{id} has no published price");
    }
}

/// The video models Runway resells on `POST /v1/text_to_video` and
/// `/v1/image_to_video`, carried as runway routes beside each vendor's own
/// (the hub-route policy, 2026-11-04). Each row matches both of its model's
/// branches:
/// - the tighter `promptText` maxLength;
/// - the aspects and tiers of its `ratio` enum (or its `resolution` field):
///   every aspect at every tier, video/runway.rs mapping each to its size;
/// - the duration bounds both modes share (Veo "4, 6, 8", Kling 2.5 "5, 10");
/// - a last frame where `promptImage` takes position "last";
/// - its `references` maxItems;
/// - the price of its default request, at "$0.01 per credit".
///
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-11-10)
/// @see <https://docs.dev.runwayml.com/guides/pricing.md> (2026-11-10)
#[test]
fn runway_resold_video_rows_are_carried_with_runways_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    struct Row {
        id: &'static str,
        prompt_max: usize,
        aspects: &'static [&'static str],
        resolutions: &'static [&'static str],
        duration: (i64, i64, i64),
        last_frame: bool,
        references: u32,
        base_usd: f64,
    }
    let reg = registry();
    for r in [
        Row { id: "runway/veo3.1", prompt_max: 1000, aspects: &["9:16", "16:9"], resolutions: &["720p", "1080p"], duration: (4, 8, 8), last_frame: true, references: 0, base_usd: 3.2 },
        Row { id: "runway/veo3.1_fast", prompt_max: 1000, aspects: &["9:16", "16:9"], resolutions: &["720p", "1080p"], duration: (4, 8, 8), last_frame: true, references: 0, base_usd: 1.2 },
        Row { id: "runway/gemini_omni_flash", prompt_max: 4000, aspects: &["9:16", "16:9"], resolutions: &[], duration: (3, 10, 5), last_frame: false, references: 0, base_usd: 0.5 },
        Row { id: "runway/gemini_omni_flash_1.1", prompt_max: 4000, aspects: &["9:16", "16:9"], resolutions: &["360p", "720p", "1080p", "4k"], duration: (3, 10, 5), last_frame: true, references: 5, base_usd: 0.5 },
        Row { id: "runway/gemini_omni_flash_1.2", prompt_max: 4000, aspects: &["9:16", "16:9"], resolutions: &["360p", "720p", "1080p", "4k"], duration: (3, 15, 5), last_frame: true, references: 5, base_usd: 0.35 },
        Row { id: "runway/kling2.5_turbo_pro", prompt_max: 2500, aspects: &["1:1", "9:16", "16:9"], resolutions: &["720p", "1080p"], duration: (5, 10, 5), last_frame: true, references: 0, base_usd: 0.6 },
        Row { id: "runway/kling3.0_standard", prompt_max: 2500, aspects: &["1:1", "9:16", "16:9"], resolutions: &[], duration: (3, 15, 5), last_frame: true, references: 0, base_usd: 0.65 },
        Row { id: "runway/kling3.0_pro", prompt_max: 2500, aspects: &["1:1", "9:16", "16:9"], resolutions: &[], duration: (3, 15, 5), last_frame: true, references: 0, base_usd: 0.85 },
        Row { id: "runway/kling3.0_4k", prompt_max: 2500, aspects: &["1:1", "9:16", "16:9"], resolutions: &[], duration: (3, 15, 5), last_frame: true, references: 0, base_usd: 2.15 },
        Row { id: "runway/klingO3_standard", prompt_max: 2500, aspects: &["1:1", "9:16", "16:9"], resolutions: &[], duration: (3, 15, 5), last_frame: true, references: 7, base_usd: 0.65 },
        Row { id: "runway/klingO3_pro", prompt_max: 2500, aspects: &["1:1", "9:16", "16:9"], resolutions: &[], duration: (3, 15, 5), last_frame: true, references: 7, base_usd: 0.85 },
        Row { id: "runway/klingO3_4k", prompt_max: 2500, aspects: &["1:1", "9:16", "16:9"], resolutions: &[], duration: (3, 15, 5), last_frame: true, references: 7, base_usd: 2.15 },
        Row { id: "runway/kling_4", prompt_max: 26000, aspects: &["1:1", "9:16", "16:9", "21:9"], resolutions: &["720p", "1080p"], duration: (3, 30, 10), last_frame: true, references: 10, base_usd: 2.35 },
        Row { id: "runway/kling_4_flash", prompt_max: 26000, aspects: &["1:1", "9:16", "16:9", "21:9"], resolutions: &[], duration: (3, 20, 5), last_frame: false, references: 10, base_usd: 0.47 },
        Row { id: "runway/hailuo3", prompt_max: 6000, aspects: &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"], resolutions: &["768P", "2K"], duration: (5, 15, 5), last_frame: true, references: 9, base_usd: 0.5 },
        Row { id: "runway/h3_max", prompt_max: 6000, aspects: &[], resolutions: &["480P", "768P"], duration: (5, 15, 5), last_frame: true, references: 0, base_usd: 0.4 },
        Row { id: "runway/seedance2", prompt_max: 10000, aspects: &["1:1", "3:4", "4:3", "9:16", "16:9", "21:9"], resolutions: &["480p", "720p", "1080p", "4k"], duration: (4, 15, 5), last_frame: true, references: 9, base_usd: 1.5 },
        Row { id: "runway/seedance2_fast", prompt_max: 10000, aspects: &["1:1", "3:4", "4:3", "9:16", "16:9", "21:9"], resolutions: &["480p", "720p"], duration: (4, 15, 5), last_frame: true, references: 9, base_usd: 1.2 },
        Row { id: "runway/seedance2_mini", prompt_max: 10000, aspects: &["1:1", "3:4", "4:3", "9:16", "16:9", "21:9"], resolutions: &["480p", "720p"], duration: (4, 15, 5), last_frame: true, references: 9, base_usd: 0.75 },
        Row { id: "runway/seedance2_5", prompt_max: 15000, aspects: &["1:1", "3:4", "4:3", "9:16", "16:9", "21:9"], resolutions: &["480p", "720p", "1080p"], duration: (4, 30, 5), last_frame: true, references: 30, base_usd: 1.5 },
        Row { id: "runway/grok_imagine_1_5", prompt_max: 2500, aspects: &["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3"], resolutions: &["480p", "720p", "1080p"], duration: (1, 15, 5), last_frame: false, references: 7, base_usd: 0.8 },
        Row { id: "runway/grok_imagine_1_5_lite", prompt_max: 2500, aspects: &["1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9"], resolutions: &["480p", "720p", "1080p"], duration: (1, 15, 5), last_frame: false, references: 0, base_usd: 0.15 },
        Row { id: "runway/wan3", prompt_max: 20000, aspects: &["1:1", "3:4", "4:3", "9:16", "16:9"], resolutions: &["480p", "720p", "1080p"], duration: (2, 30, 5), last_frame: true, references: 10, base_usd: 0.5 },
        Row { id: "runway/wan3_prime", prompt_max: 20000, aspects: &["1:1", "3:4", "4:3", "9:16", "16:9"], resolutions: &["480p", "720p", "1080p"], duration: (2, 30, 5), last_frame: true, references: 10, base_usd: 0.7 },
        Row { id: "runway/happyhorse_1_0", prompt_max: 2500, aspects: &["1:1", "3:4", "4:3", "9:16", "16:9"], resolutions: &["720p", "1080p"], duration: (3, 15, 5), last_frame: false, references: 0, base_usd: 0.75 },
    ] {
        let id = r.id;
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(m.provider == "runway" && m.media_type == litegen::capabilities::MediaType::Video, "{id}");
        assert!(m.capabilities.text_to_video && m.capabilities.image_to_video, "{id}: both modes");
        assert_eq!(m.prompt.max_length, Some(r.prompt_max), "{id}: promptText maxLength");
        assert_eq!(m.docs_url.as_deref(), Some("https://docs.dev.runwayml.com/api/#tag/Start-generating/paths/~1v1~1image_to_video/post"), "{id}");
        assert_eq!(aspect_ratios(&reg, id), r.aspects, "{id}: the aspects its ratio enum covers");
        assert_eq!(enum_values(&reg, id, "resolution"), r.resolutions, "{id}: its tiers");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(r.duration.0), Some(r.duration.1), Some(r.duration.2)), "{id}: duration"),
            other => panic!("{id} duration_seconds: {other:?}"),
        }
        let roles = &m.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id}: no ref_inputs")).roles;
        assert_eq!(roles.get("first_frame").map(|role| role.max_count), Some(1), "{id}: a start frame");
        assert_eq!(roles.contains_key("last_frame"), r.last_frame, "{id}: promptImage position \"last\"");
        assert_eq!(roles.get("reference").map_or(0, |role| role.max_count), r.references, "{id}: references maxItems");
        assert!((m.pricing.base_cost_usd - r.base_usd).abs() < 1e-9, "{id}: {} (want {})", m.pricing.base_cost_usd, r.base_usd);
    }
    // Discrete durations: Veo "enum": [4, 6, 8]; Kling 2.5 Turbo Pro [5, 10].
    for (id, values) in [("runway/veo3.1", vec![4, 6, 8]), ("runway/veo3.1_fast", vec![4, 6, 8]), ("runway/kling2.5_turbo_pro", vec![5, 10])] {
        assert!(matches!(reg.get(id).unwrap().params.get("duration_seconds"), Some(ParamSpec::Int(d)) if d.values == values), "{id}");
    }

    // Prices past the default, read from the guide.
    let price = |id: &str, params: serde_json::Value, roles: &[&str]| {
        let m = reg.get(id).unwrap();
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        req.as_object_mut().unwrap().extend(params.as_object().unwrap().clone());
        req["reference_images"] = roles
            .iter()
            .enumerate()
            .map(|(i, role)| serde_json::json!({ "type": "url", "value": format!("https://example.com/{i}.png"), "role": role }))
            .collect();
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    use serde_json::json as j;
    for (id, params, roles, usd) in [
        // "| `veo3.1 (no audio)` | 20 credits per second"; fast "(no audio) | 10".
        ("runway/veo3.1", j!({ "audio": false }), &[][..], 1.6),
        ("runway/veo3.1_fast", j!({ "audio": false, "duration_seconds": 4 }), &[][..], 0.4),
        // "Kling 3.0 Standard and Pro use the no-audio rate when `audio` is `false`"; O3 does not.
        ("runway/kling3.0_pro", j!({ "audio": false, "duration_seconds": 10 }), &[][..], 1.2),
        ("runway/klingO3_pro", j!({ "audio": false }), &[][..], 0.85),
        ("runway/kling_4", j!({ "resolution": "720p", "duration_seconds": 5 }), &[][..], 0.785),
        // hailuo3 "15 credits per second, plus 2 credits per reference image".
        ("runway/hailuo3", j!({ "resolution": "2K", "duration_seconds": 10 }), &["reference", "reference", "reference"][..], 1.56),
        ("runway/h3_max", j!({ "resolution": "480P" }), &[][..], 0.25),
        ("runway/seedance2", j!({ "resolution": "4k" }), &[][..], 7.5),
        // Seedance 2.5 "A preview (`draft: true` …) bills at the 480p rate".
        ("runway/seedance2_5", j!({ "extra": { "draft": true } }), &[][..], 1.0),
        ("runway/seedance2_5", j!({ "resolution": "1080p", "duration_seconds": 10 }), &[][..], 6.8),
        ("runway/gemini_omni_flash", j!({}), &["first_frame"][..], 0.51),
        ("runway/gemini_omni_flash_1.1", j!({ "resolution": "4k" }), &["first_frame"][..], 1.51),
        ("runway/gemini_omni_flash_1.2", j!({ "resolution": "360p", "duration_seconds": 3 }), &["reference", "reference"][..], 0.08),
        ("runway/grok_imagine_1_5", j!({ "resolution": "1080p" }), &["reference"][..], 1.46),
        ("runway/grok_imagine_1_5_lite", j!({ "resolution": "480p" }), &["first_frame"][..], 0.11),
        ("runway/wan3_prime", j!({ "resolution": "1080p", "duration_seconds": 10 }), &[][..], 2.8),
        ("runway/happyhorse_1_0", j!({ "resolution": "1080p" }), &[][..], 1.5),
    ] {
        let got = price(id, params.clone(), roles);
        assert!((got - usd).abs() < 1e-9, "{id} {params} {roles:?}: {got} (want {usd})");
    }
}

/// #9: gen4.5's `outputFormat` turns the output into "a ProRes .mov", "a .zip
/// of PNG frames" or a 10-bit .mp4, each with "an additional per-second credit
/// surcharge: 5 credits per second for `prores` and `png_sequence`, and 20
/// credits per second for every 10-bit and deeper profile". The row prices,
/// labels and delivers an H.264 .mp4, so neither it nor `proresProfile` is a
/// caller's knob until each format is priced and delivered as what it is.
/// @see <https://docs.dev.runwayml.com/openapi.json> (POST /v1/text_to_video, 2026-10-30)
/// @see <https://docs.dev.runwayml.com/guides/pricing.md> (2026-10-30)
#[test]
fn runway_gen4_5_returns_only_the_mp4_it_prices() {
    let reg = registry();
    let schema = reg.get("runway/gen4.5").expect("runway/gen4.5 missing");
    for key in ["outputFormat", "proresProfile"] {
        assert!(!schema.extra_allowlist.iter().any(|k| k == key), "runway/gen4.5 allowlists {key:?}: {:?}", schema.extra_allowlist);
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

/// Gemini 3 Pro Image and Gemini 2.5 Flash Image document the same 10 aspect
/// ratios: 4:5 and 5:4 yes, the 1:4/4:1/1:8/8:1 extremes no. Pro's table is
/// headed "3.1 Pro Image", but its 1K/2K/4K sizes and 1120/2000 tokens are
/// gemini-3-pro-image's. 2.5 Flash gained 4:5 and 5:4 by 2026-10-12.
/// @see <https://ai.google.dev/gemini-api/docs/generate-content/image-generation#aspect_ratios_and_image_size> (2026-10-12)
#[test]
fn gemini_3_pro_and_2_5_flash_image_advertise_the_ten_documented_aspect_ratios() {
    const DOCUMENTED: [&str; 10] = [
        "1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9",
    ];
    let mut documented: Vec<String> = DOCUMENTED.iter().map(|s| s.to_string()).collect();
    documented.sort();
    let reg = registry();
    for id in ["google/gemini-3-pro-image", "google/gemini-2.5-flash-image"] {
        let mut allowed = aspect_ratios(&reg, id);
        allowed.sort();
        assert_eq!(allowed, documented, "{id} aspect ratios differ from Google's table");
    }
}

/// Gemini 2.5 Flash Image: "Maximum images per prompt: 3" (Google's model
/// page) and "`gemini-2.5-flash-image` works best with up to 3 images as
/// input" (the Gemini API image page). The row took one.
/// @see <https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/gemini/2-5-flash-image> (2026-11-03)
/// @see <https://ai.google.dev/gemini-api/docs/image-generation> (2026-11-03)
#[test]
fn gemini_2_5_flash_image_takes_three_input_images() {
    let reg = registry();
    let refs = reg
        .get("google/gemini-2.5-flash-image")
        .expect("gemini-2.5-flash-image missing")
        .ref_inputs
        .as_ref()
        .expect("gemini-2.5-flash-image declares no ref_inputs");
    assert_eq!(refs.max_total, 3, "up to 3 input images");
    assert_eq!(refs.roles.get("init").map(|r| r.max_count), Some(3), "the init role must allow all 3");
}

/// No Gemini image row declares `seed`. `GenerationConfig.seed` (int32) is a
/// generic generateContent field, but Google's image-model pages name only
/// "Temperature: 0.0-2.0 (default 1.0) topP: 0.0-1.0 (default 0.95) topK: 64
/// (fixed) candidateCount: 1" and promise no reproducible image: advertising a
/// seed promises what the vendor does not document (the conservative reading).
/// @see <https://docs.cloud.google.com/gemini-enterprise-agent-platform/models/gemini/2-5-flash-image> (2026-11-03)
#[test]
fn gemini_image_rows_declare_no_seed() {
    let reg = registry();
    let rows: Vec<_> = reg.served().into_iter().filter(|m| m.provider == "google").collect();
    assert!(!rows.is_empty());
    for m in rows {
        assert!(!m.params.contains_key("seed"), "{} declares a seed Google does not document for its image models", m.id);
    }
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

/// Recraft V2 / V3 raster sizes are the Appendix's V2/V3 column, all 14 of it:
/// "Exact size `WxH` … Must be one of the sizes listed for the model; any other
/// value is rejected." The OpenAPI `ImageSize` enum also lists 1707x1024 (10:6),
/// which the column doesn't: the vendor contradicts itself, so the conservative
/// reading (the per-model column) holds.
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("Image sizes"), read 2026-11-03
/// @see <https://external.api.recraft.ai/doc/spec/external-api.yaml> (ImageSize), read 2026-11-03
#[test]
fn recraft_v2_v3_sizes_are_the_v2_v3_table() {
    let mut v2_v3: Vec<(u32, u32)> = vec![
        (1024, 1024), (2048, 1024), (1024, 2048), (1536, 1024), (1024, 1536), (1365, 1024),
        (1024, 1365), (1280, 1024), (1024, 1280), (1024, 1707), (1434, 1024), (1024, 1434),
        (1820, 1024), (1024, 1820),
    ];
    v2_v3.sort();
    let reg = registry();
    for id in ["recraft/recraftv3", "recraft/recraftv2"] {
        assert_eq!(recraft_sizes(&reg, id), v2_v3, "{id}");
    }
}

/// Recraft V4.1 Flash (new 2026-11-03): "The fastest and cheapest Recraft API
/// model: about 1.3 seconds and $0.007 per image … Raster only, no styles";
/// `model` "recraftv4_1_flash", prompt "Up to 10,000 characters", the 1K
/// sizes of V4.1, `random_seed` and `controls.colors`; "Not supported by
/// Flash: `style`, `style_id`, `style_match`, style references,
/// `negative_prompt`, and `text_layout`".
/// @see <https://www.recraft.ai/docs/api-reference/models/recraft-v4-1-flash.md> (2026-11-03)
/// @see <https://www.recraft.ai/docs/api-reference/pricing.md> (2026-11-03)
#[test]
fn recraft_v4_1_flash_is_carried_with_its_documented_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let flash = reg.get("recraft/recraftv4_1_flash").expect("recraftv4_1_flash missing");
    let v41 = reg.get("recraft/recraftv4_1").expect("recraftv4_1 missing");
    assert!((flash.pricing.base_cost_usd - 0.007).abs() < 1e-9);
    assert!(flash.capabilities.text_to_image && !flash.capabilities.image_to_image, "generate image only");
    assert_eq!(flash.prompt.max_length, Some(10000));
    for k in ["style", "negative_prompt"] {
        assert!(!flash.params.contains_key(k), "Flash takes no {k}");
    }
    assert_eq!(flash.extra_allowlist, ["controls"], "no style_id on Flash");
    assert!(matches!(flash.params.get("seed"), Some(ParamSpec::Seed(_))) && flash.params.contains_key("palette"));
    match (flash.params.get("size"), v41.params.get("size")) {
        (Some(ParamSpec::Size(SizeSpec::Enum(a))), Some(ParamSpec::Size(SizeSpec::Enum(b)))) => assert_eq!(a.values, b.values, "the 1K table"),
        other => panic!("{other:?}"),
    }
}

/// Every Recraft generation model takes an aspect ratio as `size: "w:h"`, the
/// Appendix's 14 ratios: "**Aspect ratio** `w:h`, for example `16:9`. Works
/// with every model. Prefer it." Vector models take only that form.
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> ("Image sizes"), read 2026-11-03
#[test]
fn every_recraft_generation_model_takes_the_appendix_aspect_ratios() {
    use litegen::capabilities::ParamSpec;
    let ratios = ["1:1", "2:1", "1:2", "3:2", "2:3", "4:3", "3:4", "5:4", "4:5", "6:10", "14:10", "10:14", "16:9", "9:16"];
    let reg = registry();
    // The text-to-image rows; remove- and replace-background take an image.
    for m in reg.for_provider("recraft").filter(|m| m.capabilities.text_to_image) {
        match m.params.get("aspect_ratio") {
            Some(ParamSpec::AspectRatio(a)) => assert_eq!(a.allowed, ratios, "{}", m.id),
            other => panic!("{} takes no aspect_ratio: {other:?}", m.id),
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

/// Q1 now takes 3:4 and 4:3 ("Accepted values: 16:9, 9:16, 3:4, 4:3, 1:1",
/// text2video; reference2video the same plus auto); vidu2.0, which no
/// endpoint page documents any more, keeps 16:9, 9:16 and 1:1.
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-10-30)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/reference-to-video.md> (2026-10-30)
#[test]
fn vidu_3_4_and_4_3_are_only_advertised_for_q1_and_q2_models() {
    let reg = registry();
    let ratios = aspect_ratios(&reg, "vidu/vidu2.0");
    assert!(!ratios.is_empty(), "vidu/vidu2.0 declares no aspect ratios");
    for ar in ratios {
        assert!(["16:9", "9:16", "1:1"].contains(&ar.as_str()), "vidu/vidu2.0 allows {ar:?}");
    }
    let mut q1 = aspect_ratios(&reg, "vidu/viduq1");
    q1.sort();
    assert_eq!(q1, ["16:9", "1:1", "3:4", "4:3", "9:16"]);
    for ar in aspect_ratios(&reg, "vidu/viduq2-pro") {
        assert!(
            ["16:9", "9:16", "3:4", "4:3", "1:1"].contains(&ar.as_str()),
            "vidu/viduq2-pro allows {ar:?}, not in reference2video's aspect_ratio list"
        );
    }
}

/// No current Vidu page documents `style`: Q1's text2video body is model,
/// prompt, duration, seed, aspect_ratio, resolution, audio, payload,
/// off_peak, watermark, wm_position, wm_url, meta_data and callback_url.
/// `off_peak` is allowlisted nowhere: "Tasks submitted in off-peak mode will
/// be generated within 48 hours", while litegen fails a job still running
/// after 2 hours (and bills it). Q2's bodies have no `bgm` or
/// `movement_amplitude`.
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/text-to-video.md> (2026-10-30)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/image-to-video.md> (2026-10-30)
#[test]
fn vidu_rows_allowlist_only_fields_their_bodies_take() {
    let reg = registry();
    for m in reg.for_provider("vidu") {
        for k in ["style", "off_peak"] {
            assert!(!m.extra_allowlist.iter().any(|a| a == k), "{} allowlists `{k}`", m.id);
        }
    }
    // E4a: `callback_url` was their only key, and it is litegen's.
    for id in ["vidu/viduq2-pro", "vidu/viduq2-pro-fast"] {
        assert!(reg.get(id).unwrap().extra_allowlist.is_empty(), "{id}");
    }
}

/// E4b, N3: a price compares the extra it reads exactly as sent, so every
/// extra a pricing rule reads is typed (`true` and `"true"` are not one
/// value): fal/hyper3d-rodin `addons`, runway/ideogram_4_5 `preciseEdit`,
/// runway/seedance2_5 `draft`.
#[test]
fn every_extra_a_price_reads_is_typed() {
    let reg = registry();
    let mut read = Vec::new();
    for m in reg.all() {
        let served = serde_json::to_value(m).unwrap();
        for rule in &m.pricing.variable_pricing {
            for key in rule.when.keys().filter_map(|k| k.strip_prefix("extra.")) {
                let kind = served["extra_params"][key]["kind"].as_str().map(str::to_string);
                read.push((m.id.clone(), key.to_string(), kind));
            }
        }
    }
    read.sort();
    let want: Vec<(String, String, Option<String>)> = [
        ("fal/hyper3d-rodin", "addons", "string"),
        ("runway/ideogram_4_5", "preciseEdit", "bool"),
        ("runway/seedance2_5", "draft", "bool"),
    ]
    .iter()
    .map(|(id, key, kind)| (id.to_string(), key.to_string(), Some(kind.to_string())))
    .collect();
    assert_eq!(read, want);
}

/// E4c: `safety_tolerance` is a string on fal's FLUX.1 [pro] and FLUX.2 rows
/// ("1"-"6", "1"-"5"), an integer on fal's FLUX 3 rows (0-4), and an
/// integer on BFL (0-6 or 0-7, per endpoint) and Replicate. A client cannot
/// tell from the name, so each row types it as its own endpoint takes it.
#[test]
fn safety_tolerance_is_typed_as_each_vendor_takes_it() {
    let reg = registry();
    let mut seen = Vec::new();
    for m in reg.all().filter(|m| m.provider != "mock" && m.extra_allowlist.iter().any(|k| k == "safety_tolerance")) {
        let t = serde_json::to_value(m).unwrap()["extra_params"]["safety_tolerance"].clone();
        let want = match m.id.as_str() {
            "fal/flux-pro" | "fal/flux-kontext-pro" | "fal/flux-kontext-max" => serde_json::json!({ "kind": "string", "enum_values": ["1", "2", "3", "4", "5", "6"], "default": "2" }),
            "fal/flux-2-pro" | "fal/flux-2-max" | "fal/flux-2-flex" => serde_json::json!({ "kind": "string", "enum_values": ["1", "2", "3", "4", "5"], "default": "2" }),
            id if id.starts_with("fal/flux-3-") => serde_json::json!({ "kind": "int", "min": 0, "max": 4, "default": 2 }),
            id if id.starts_with("fal/") => serde_json::json!({ "kind": "string" }),
            "bfl/flux-2-pro" | "bfl/flux-2-max" | "bfl/flux-2-klein-9b" | "bfl/flux-2-flex" | "bfl/flux-3-image" | "bfl/flux-3-image-klein" => {
                serde_json::json!({ "kind": "int", "min": 0, "max": 7, "default": 2 })
            }
            id if id.starts_with("bfl/") => serde_json::json!({ "kind": "int", "min": 0, "max": 6, "default": 2 }),
            "replicate/flux-3-image" => serde_json::json!({ "kind": "int", "min": 0, "max": 4 }),
            id if id.starts_with("replicate/") => serde_json::json!({ "kind": "int", "min": 1, "max": 6, "default": 2 }),
            id => panic!("{id} allowlists safety_tolerance; say how its endpoint takes it"),
        };
        // The served spec carries `values: []`-style empties only when set.
        let got: serde_json::Map<String, serde_json::Value> = t.as_object().cloned().unwrap_or_default().into_iter().filter(|(_, v)| !v.is_null() && v != &serde_json::json!([])).collect();
        assert_eq!(serde_json::Value::Object(got), want, "{}", m.id);
        seen.push(m.id.clone());
    }
    assert_eq!(seen.len(), 27, "{seen:?}");
}

/// E4c: every key a vendor row allowlists is typed in `extra_params`, from
/// evidence in the repo (a drift snapshot of the vendor's schema, the row's
/// own comment quoting the vendor, or the adapter and its tests), except
/// these: values that are JSON objects or integer lists, which no extra kind
/// describes, and keys with no evidence of their type. The mock rows stay
/// untyped: they test passthrough of any value.
#[test]
fn every_allowlisted_extra_is_typed_or_named_untyped() {
    const UNTYPED: &[(&str, &str)] = &[
        ("background_color", "an object {r, g, b} (fal Recraft V4)"),
        ("bbox_condition", "an integer list (fal Rodin)"),
        ("color_palette", "an object (Ideogram 3.0)"),
        ("controls", "an object (Recraft)"),
        ("contentModeration", "an object (Runway)"),
        ("providerSettings", "an object (Runway)"),
        ("optimize_prompt_options", "an object (TokenHub Seedream)"),
        ("use_search_tool", "an object (TokenHub Hunyuan)"),
        ("Style", "no evidence (Hunyuan image)"),
        ("contextWindow", "no evidence (Runway Ideogram 4.5)"),
        ("promptExpansionMode", "no evidence (Runway H3 Max)"),
        ("thinkingLevel", "no evidence (Runway Nano Banana 2.1)"),
        ("camera_movement", "no evidence (TokenHub PixVerse)"),
    ];
    let reg = registry();
    let (mut typed, mut untyped, mut missing) = (0, 0, Vec::new());
    for m in reg.all().filter(|m| m.provider != "mock") {
        let served = serde_json::to_value(m).unwrap();
        for key in &m.extra_allowlist {
            if served["extra_params"][key]["kind"].is_string() {
                typed += 1;
            } else if UNTYPED.iter().any(|(k, _)| k == key) {
                untyped += 1;
            } else {
                missing.push(format!("{} {key}", m.id));
            }
        }
    }
    assert!(missing.is_empty(), "allowlisted with no type:\n{}", missing.join("\n"));
    assert_eq!((typed, untyped), (208 + 3, 47), "typed and untyped allowlist entries");
}

/// D2: a param description is a whole sentence. In a YAML flow mapping a
/// plain scalar ends at ", ", so an unquoted description with a comma was cut
/// there (and the rest became a key); the twelve such lines are quoted.
#[test]
fn every_param_description_is_a_whole_sentence() {
    let reg = registry();
    let mut cut = Vec::new();
    for m in reg.all() {
        for (name, spec) in &m.params {
            let v = serde_json::to_value(spec).unwrap();
            if let Some(d) = v["description"].as_str() {
                if !d.trim_end().ends_with(['.', ')', '!', '?', '"', '\'', '\u{201d}', '\u{2019}']) {
                    cut.push(format!("{} {name}: {d:?}", m.id));
                }
            }
        }
    }
    cut.sort();
    assert!(cut.is_empty(), "{} descriptions end mid-sentence:\n{}", cut.len(), cut.join("\n"));
}

/// viduq2-pro: 540p/720p/1080p and 1-10s on img2video, start-end2video and
/// reference2video (start-end2video went from 1-8s to 1-10s by 2026-10-12).
/// img2video, where a lone first frame routes, caps the prompt at 2000
/// characters (5000 elsewhere); one limit serves every endpoint.
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/image-to-video.md>
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/start-end-to-video.md>
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/reference-to-video.md>, read 2026-10-12
#[test]
fn every_vidu_row_takes_whole_seconds() {
    // C5: "| `duration` | `integer` |" on every Vidu page. Declared in whole
    // seconds, a fraction is rounded before the price — never billed as 5.5 s
    // and sent as 5.
    let reg = registry();
    let vidu: Vec<_> = reg.served().into_iter().filter(|m| m.provider == "vidu").collect();
    assert!(!vidu.is_empty());
    for m in &vidu {
        assert!(matches!(m.params.get("duration_seconds"), Some(ParamSpec::Int(_))), "{}: {:?}", m.id, m.params.get("duration_seconds"));
    }
}

#[test]
fn vidu_q2_pro_duration_and_resolution_hold_on_every_endpoint() {
    let reg = registry();
    let schema = reg.get("vidu/viduq2-pro").expect("viduq2-pro missing");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Int(f)) => {
            assert_eq!(f.min, Some(1), "viduq2-pro accepts 1s clips");
            assert_eq!(f.max, Some(10), "every viduq2-pro endpoint goes to 10s");
        }
        other => panic!("viduq2-pro duration_seconds is {other:?}"),
    }
    assert_eq!(schema.prompt.max_length, Some(2000), "img2video caps the viduq2-pro prompt at 2000");
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

// ─── Luma: vendor feature audit, 2026-11-03 ─────────────────────────────────

/// Photon (`POST /generations/image`): "format … The format of the image",
/// the ImageFormat enum `jpg | png` ("default": "jpg"). PixelGenie L9's
/// "Luma always returns JPEG" no longer holds, so the rows offer both, and
/// `format` is litegen's to send (never an extra).
/// @see <https://docs.lumalabs.ai/reference/generateimage.md> (2026-11-03)
#[test]
fn luma_photon_returns_jpeg_or_png() {
    let reg = registry();
    for id in ["luma/photon-1", "luma/photon-flash-1"] {
        let mut formats = enum_values(&reg, id, "output_format");
        formats.sort();
        assert_eq!(formats, ["jpeg", "png"], "{id}: ImageFormat jpg | png");
        let schema = reg.get(id).unwrap();
        assert!(!schema.extra_allowlist.iter().any(|k| k == "format"), "{id}: format is derived from output_format");
    }
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

/// "Dreamina Seedance 1.0 pro: Default 5; supports [2, 12]." The field is
/// "duration `integer`" (C5): declared in whole seconds, a fraction is rounded
/// before the per-second price, never billed as 5.4 s and sent as 5.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1520757> (read 2026-09-11; integer re-read 2026-11-03)
#[test]
fn bytedance_seedance_1_0_pro_duration_range_matches_the_api() {
    let reg = registry();
    let schema = reg
        .get("bytedance/doubao-seedance-1-0-pro-250528")
        .expect("seedance-1-0-pro missing");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Int(f)) => {
            assert_eq!(f.min, Some(2), "Seedance 1.0 pro accepts 2s");
            assert_eq!(f.max, Some(12), "Seedance 1.0 pro tops out at 12s");
            assert_eq!(f.default, Some(5), "vendor default is 5s");
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

/// Both Seedance 1.0 models document resolution "Default `1080p`; supports
/// `480p`, `720p`, and `1080p`" (the catalog left 1.0 pro without a default).
/// @see <https://docs.byteplus.com/en/docs/modelark/create-video-generation-task-api> (read 2026-10-12)
#[test]
fn bytedance_seedance_1_0_resolutions_default_to_1080p() {
    let reg = registry();
    for id in ["bytedance/doubao-seedance-1-0-pro-250528", "bytedance/seedance-1-0-pro-fast-251015"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        match schema.params.get("resolution") {
            Some(ParamSpec::String(r)) => {
                assert_eq!(r.enum_values, vec!["480p", "720p", "1080p"], "{id} resolutions");
                assert_eq!(r.default.as_deref(), Some("1080p"), "{id} resolution default");
            }
            other => panic!("{id} resolution is {other:?}, expected String"),
        }
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
    // PixelGenie L10: the still camera is the canonical `camera`, sent as camera_fixed.
    assert!(
        matches!(schema.params.get("camera"), Some(ParamSpec::String(c)) if c.enum_values == ["fixed", "auto"]),
        "camera fixed | auto is sent as the documented camera_fixed body field"
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

// ─── fal: vendor feature audit, 2026-11-03 ──────────────────────────────────

/// fal-ai/fast-sdxl's "format" ("enum": ["jpeg", "png"], default "jpeg") is
/// the canonical `output_format`; the row declared none.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/fast-sdxl> (2026-11-03)
#[test]
fn fal_fast_sdxl_returns_jpeg_or_png() {
    let reg = registry();
    let mut formats = enum_values(&reg, "fal/sdxl", "output_format");
    formats.sort();
    assert_eq!(formats, ["jpeg", "png"]);
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

/// flux-dev and SDXL take an image with "prompt_strength … Prompt strength
/// when using img2img. 1.0 corresponds to full destruction of information in
/// image" (0–1, default 0.8): the canonical `strength`, which the adapter sends
/// as `prompt_strength` but neither row declared.
/// @see <https://replicate.com/black-forest-labs/flux-dev/api/schema> (2026-11-03)
/// @see <https://replicate.com/stability-ai/sdxl/api/schema> (2026-11-03)
#[test]
fn replicate_img2img_rows_take_a_strength() {
    let reg = registry();
    for id in ["replicate/flux-dev", "replicate/sdxl"] {
        match reg.get(id).unwrap_or_else(|| panic!("{id} missing")).params.get("strength") {
            Some(ParamSpec::Float(f)) => assert_eq!((f.min, f.max, f.default), (Some(0.0), Some(1.0), Some(0.8)), "{id}: prompt_strength 0-1, default 0.8"),
            other => panic!("{id} strength is {other:?}"),
        }
    }
}

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
            !schema.params.contains_key("seed"),
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

/// Each Seedream size rule is a pixel range and a ratio, not just an edge box:
/// 4.0 "Total pixels range: [`1280x720=921,600`, `4096x4096=16,777,216`]",
/// 4.5 and 5.0 lite "[`2560x1440`(3,686,400), `4096x4096`(16,777,216)]", all
/// with "Aspect ratio range: [1/16, 16]". The box alone let through the size
/// the page calls invalid ("Invalid example: `1500x1500`"), which the vendor
/// then refused. Every documented valid size must pass the declared rule, and
/// the invalid one must not.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1541523> (2026-11-03)
#[test]
fn bytedance_seedream_sizes_carry_the_vendors_pixel_range_and_ratio() {
    use litegen::capabilities::schema::SizeSpec;
    let fits = |f: &litegen::capabilities::schema::SizeSpecFreeform, w: u32, h: u32| {
        let pixels = u64::from(w) * u64::from(h);
        let aspect = f64::from(w.max(h)) / f64::from(w.min(h));
        (f.min_width..=f.max_width).contains(&w)
            && (f.min_height..=f.max_height).contains(&h)
            && f.min_pixels.is_none_or(|m| pixels >= m)
            && f.max_pixels.is_none_or(|m| pixels <= m)
            && f.max_aspect.is_none_or(|m| aspect <= m + 1e-9)
    };
    let reg = registry();
    for (id, min_pixels, valid, invalid) in [
        ("bytedance/seedream-4-0-250828", 921_600u64, &[(1280u32, 720u32), (1600, 600), (1512, 648), (6240, 2656)][..], &[(800u32, 800u32), (4097, 4097)][..]),
        ("bytedance/seedream-4-5-251128", 3_686_400, &[(2560, 1440), (3750, 1250), (2048, 2048), (6240, 2656)][..], &[(1500, 1500), (1024, 1024)][..]),
        ("bytedance/seedream-5-0-lite-260128", 3_686_400, &[(2560, 1440), (3750, 1250), (2048, 2048), (6240, 2656)][..], &[(1500, 1500), (1024, 1024)][..]),
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing from the catalog"));
        let Some(ParamSpec::Size(SizeSpec::Freeform(f))) = schema.params.get("size") else {
            panic!("{id} size is not a freeform WxH size");
        };
        assert_eq!(f.min_pixels, Some(min_pixels), "{id}: the vendor's pixel floor");
        assert_eq!(f.max_pixels, Some(16_777_216), "{id}: the vendor's pixel cap, 4096x4096");
        assert_eq!(f.max_aspect, Some(16.0), "{id}: aspect ratio range [1/16, 16]");
        for &(w, h) in valid {
            assert!(fits(f, w, h), "{id} refuses the documented size {w}x{h}");
        }
        for &(w, h) in invalid {
            assert!(!fits(f, w, h), "{id} admits {w}x{h}, outside the vendor's pixel range");
        }
        // 16:1 is the widest the vendor makes; 17:1 is not.
        assert!(!fits(f, 17 * 512, 512), "{id} admits 17:1");
    }
}

/// "output_format `string` `Default: jpeg` · values: `png`, `jpeg` ·
/// supported models: Seedream 5.0 lite". The other Seedream rows must not
/// promise it: their endpoint ignores the field.
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1541523> (2026-11-03)
#[test]
fn bytedance_only_seedream_5_0_lite_takes_an_output_format() {
    let reg = registry();
    let lite = reg.get("bytedance/seedream-5-0-lite-260128").expect("seedream-5-0-lite missing");
    match lite.params.get("output_format") {
        Some(ParamSpec::String(s)) => {
            assert_eq!(s.enum_values, vec!["png".to_string(), "jpeg".to_string()], "png or jpeg");
            assert_eq!(s.default.as_deref(), Some("jpeg"), "the vendor's default is jpeg");
        }
        other => panic!("seedream-5-0-lite output_format is {other:?}, expected png|jpeg"),
    }
    for id in ["bytedance/seedream-4-0-250828", "bytedance/seedream-4-5-251128"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing from the catalog"));
        assert!(!schema.params.contains_key("output_format"), "{id}: output_format is a Seedream 5.0 lite field");
    }
}

/// Seedream 5.0 pro and 5.0 flash on the same Image generation API (new
/// 2026-11-03): "Seedream 5.0 pro and Seedream 5.0 flash support up to 10
/// reference images"; WxH "Total pixels range: [`1280x720`(921,600),
/// `2048x2048x1.1025`(4,624,220)]" and "Aspect ratio range: [1/16, 16]";
/// `output_format` png | jpeg; no `seed` (the page documents none). Prices:
/// "dola-seedream-5-0-flash-260915 | Free | $0.018" and pro "More than 2.61
/// million pixels (higher than 1.5K): $0.09" for the default 2K.
/// @see <https://docs.byteplus.com/en/docs/modelark/image-generation-api> (2026-11-03)
/// @see <https://docs.byteplus.com/en/docs/ModelArk/1544106> (2026-11-03)
#[test]
fn bytedance_seedream_5_0_pro_and_flash_are_carried_with_their_documented_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    for (id, price) in [("bytedance/dola-seedream-5-0-pro-260628", 0.09), ("bytedance/dola-seedream-5-0-flash-260915", 0.018)] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(schema.capabilities.text_to_image && schema.capabilities.image_to_image, "{id}");
        let Some(ParamSpec::Size(SizeSpec::Freeform(f))) = schema.params.get("size") else { panic!("{id} size") };
        assert_eq!((f.min_pixels, f.max_pixels, f.max_aspect), (Some(921_600), Some(4_624_220), Some(16.0)), "{id}");
        assert_eq!((f.min_width, f.max_width), (240, 8601), "{id}: the pixel rule's extremes");
        assert_eq!(schema.ref_inputs.as_ref().unwrap().max_total, 10, "{id}: up to 10 references");
        let mut formats = enum_values(&reg, id, "output_format");
        formats.sort();
        assert_eq!(formats, ["jpeg", "png"], "{id}");
        assert!(!schema.params.contains_key("seed"), "{id}");
        assert!((schema.pricing.base_cost_usd - price).abs() < 1e-9, "{id}");
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
        // "duration `integer`" (C5): whole seconds.
        Some(ParamSpec::Int(f)) => assert_eq!((f.min, f.max), (Some(2), Some(12)), "duration [2, 12]"),
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
        // "duration `integer`" (C5): whole seconds.
        Some(ParamSpec::Int(f)) => assert_eq!((f.min, f.max), (Some(4), Some(15)), "duration [4, 15]"),
        other => panic!("{id} duration_seconds is {other:?}"),
    }
    assert!(!schema.params.contains_key("seed"), "seed is not a 2.0-series parameter");
}

// ─── kling: 2026-09-11 drift decisions applied ──────────────────────────────

/// Kling Image 2.1 on POST /v1/images/generations (`model_name` kling-v2-1):
/// text + image input, eight aspect ratios including 21:9, prompt at most
/// 2500 characters. `image_fidelity` and `human_fidelity`: "Only kling-v2-1
/// supports this parameter" (2026-10-12; they were kling-v1/v1-5 only before
/// those retired), so 2.1 allowlists both and Image 3.0 neither.
/// @see https://kling.ai/document-api/api/image/2-1/image-generation.md (2026-10-12)
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
    let v3 = reg.get("kling/kling-v3").expect("kling/kling-v3 missing");
    for key in ["image_fidelity", "human_fidelity"] {
        assert!(schema.extra_allowlist.iter().any(|k| k == key), "kling-v2-1 supports {key}");
        assert!(!v3.extra_allowlist.iter().any(|k| k == key), "{key} is kling-v2-1 only");
    }
}

/// Kling 2.5 Turbo and 2.6 on the legacy text2video / image2video endpoints:
/// text and image input, 5 or 10 s (advertised as a 5-10 range), text2video
/// aspect ratios 16:9 | 9:16 | 1:1. cfg_scale ("kling-v2.x models do not
/// support this parameter") and camera_control ("Not Supported") must not be
/// allowlisted; `sound` (native audio) is a 2.6 capability only.
/// @see https://kling.ai/document-api/api/video/2-6/text-to-video.md (2026-10-30):
///   "`settings.aspect_ratio` … `16:9`, `9:16`, `1:1`", "`settings.duration` …
///   `5`, `10`" (the cfg_scale and camera_control quotes are from the legacy
///   video/1-6 and video/2-1 pages, read 2026-09-11 and since unpublished)
/// @see https://kling.ai/document-api/guides/capability-map/video.md (2026-10-30)
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
        // The capability map's "Resolution | 720P, 1080P" is the canonical
        // `resolution`, sent as the legacy `mode` (std = 720p, pro = 1080p);
        // `mode` is litegen's to send, never an extra (2026-11-03).
        let mut res = enum_values(&reg, id, "resolution");
        res.sort();
        assert_eq!(res, ["1080p", "720p"], "{id}: 720p or 1080p");
        assert!(!schema.extra_allowlist.iter().any(|k| k == "mode"), "{id}: mode is derived from resolution");
    }
    // PixelGenie L10: native audio is the canonical `audio`, sent as `sound`.
    let has_sound = |id: &str| matches!(reg.get(id).unwrap().params.get("audio"), Some(ParamSpec::Bool(_)));
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

// ─── leonardo: vendor feature audit, 2026-11-03 ─────────────────────────────

/// Motion 2.0 on `POST /generations-image-to-video`: the body's `seed` and
/// `negativePrompt`, which Leonardo's model list documents for `motion_2.0` as
/// "seed {"type":"number","minimum":0,"maximum":2147483637}" and
/// "negative_prompt {"type":"string","maxLength":1000}". The row declared
/// neither.
/// @see <https://docs.leonardo.ai/v1.0/reference/createimagetovideogeneration> (2026-11-03)
#[test]
fn leonardo_motion2_takes_a_seed_and_a_negative_prompt() {
    let reg = registry();
    let schema = reg.get("leonardo/motion2").expect("motion2 missing");
    match schema.params.get("seed") {
        Some(ParamSpec::Seed(s)) => assert_eq!((s.min, s.max), (0, 2147483637)),
        other => panic!("motion2 seed is {other:?}"),
    }
    match schema.params.get("negative_prompt") {
        Some(ParamSpec::String(s)) => assert_eq!(s.max_length, Some(1000)),
        other => panic!("motion2 negative_prompt is {other:?}"),
    }
}

/// Diffusion XL on `POST /generations`: "guidance_scale … Must be between 1
/// and 20" ("7 is recommended") and "num_inference_steps … Must be between 10
/// and 60. Default is 15" are the canonical `guidance_scale` and `steps`; the
/// row declared neither.
/// @see <https://docs.leonardo.ai/v1.0/reference/creategeneration> (2026-11-03)
#[test]
fn leonardo_diffusion_xl_takes_guidance_and_steps() {
    let reg = registry();
    let schema = reg.get("leonardo/diffusion-xl").expect("diffusion-xl missing");
    match (schema.params.get("guidance_scale"), schema.params.get("steps")) {
        (Some(ParamSpec::Float(g)), Some(ParamSpec::Int(st))) => {
            assert_eq!((g.min, g.max, g.default), (Some(1.0), Some(20.0), Some(7.0)), "guidance 1-20, 7 recommended");
            assert_eq!((st.min, st.max, st.default), (Some(10), Some(60), Some(15)), "steps 10-60, default 15");
        }
        other => panic!("diffusion-xl guidance_scale / steps are {other:?}"),
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
    // "duration {"type":"integer","enum":[4,6,8],"default":8}" (Leonardo's v2
    // model list, 2026-11-03): 5 and 7 s were advertised and then refused.
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.values, vec![4.0, 6.0, 8.0], "Veo 3.1: 4, 6 or 8 s, nothing between");
            assert_eq!(f.default, Some(8.0), "Veo 3.1 defaults to 8 s");
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

/// viduq3-turbo is in the `model` values of text2video, img2video and
/// start-end2video, so it advertises both modes and the first and last frame
/// roles, and each of those operations takes "1 - 16" (img2video,
/// start-end2video) or "available range: 1–16" (text2video) seconds. On
/// reference2video Vidu documents viduq3-turbo only as "Subjects-Reference to
/// Video", where `subjects` is "Required"; the `images` form the adapter sends
/// names `viduq3` alone ("Accepted values: `viduq3`"). So the row offers no
/// `reference` role.
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/reference-to-video.md> (2026-11-09)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video.md> (2026-11-09)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/image-to-video.md> (2026-11-09)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/start-end-to-video.md> (2026-11-09)
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
    for role in ["first_frame", "last_frame"] {
        assert!(roles.contains_key(role), "vidu/viduq3-turbo is missing the {role} role");
    }
    assert!(!roles.contains_key("reference"), "reference2video documents only the subjects form for viduq3-turbo");
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Int(f)) => {
            assert_eq!(f.min, Some(1), "text2video, img2video and start-end2video take viduq3-turbo from 1s");
            assert_eq!(f.max, Some(16), "q3 models go to 16s");
        }
        other => panic!("viduq3-turbo duration_seconds is {other:?}"),
    }
    assert_eq!(
        enum_values(&reg, "vidu/viduq3-turbo", "resolution"),
        vec!["540p".to_string(), "720p".to_string(), "1080p".to_string()],
        "viduq3-turbo resolutions"
    );
}

/// viduq3-pro's text2video, img2video and start-end2video operations all take
/// a prompt of up to 20000 characters.
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/text-to-video.md> (2026-10-12)
#[test]
fn vidu_q3_pro_prompt_limit_is_the_documented_20000() {
    let reg = registry();
    let schema = reg.get("vidu/viduq3-pro").expect("vidu/viduq3-pro missing");
    assert_eq!(schema.prompt.max_length, Some(20000), "viduq3-pro prompt is up to 20000 characters");
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
        Some(ParamSpec::Int(f)) => {
            assert_eq!(f.min, Some(1), "viduq3-pro accepts 1s clips");
            assert_eq!(f.max, Some(16), "q3 models go to 16s");
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
        // PixelGenie L10: Vidu's own `audio` is the canonical `audio` (default on for q3).
        assert!(
            matches!(schema.params.get("audio"), Some(ParamSpec::Bool(b)) if b.default == Some(true)),
            "{id} is missing audio"
        );
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == "bgm"),
            "{id} allowlists bgm, which q3 models do not support"
        );
    }
}

/// viduq2-pro-fast is only in img2video's and start-end2video's `model`
/// values: no text-to-video, no reference role, a required start frame, and
/// no aspect_ratio (neither endpoint takes one). 720p / 1080p only, and 1-10s
/// (start-end2video went from 1-8s to 1-10s by 2026-10-12).
/// @see <https://platform.vidu.com/docs/image-to-video.md>
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/start-end-to-video.md>, read 2026-10-12
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
        Some(ParamSpec::Int(f)) => {
            assert_eq!(f.min, Some(1), "viduq2-pro-fast accepts 1s clips");
            assert_eq!(f.max, Some(10), "start-end2video takes viduq2-pro-fast to 10s");
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
        // "duration: type: integer, minimum: 1, maximum: 15" (C5): declared in
        // whole seconds, a fraction is rounded before the per-second price —
        // the adapter used to truncate 5.5 s to 5 after billing 5.5 (2026-11-03).
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Int(f)) => {
                assert_eq!(f.min, Some(1), "{id} takes 1-15s");
                assert_eq!(f.max, Some(15), "{id} takes 1-15s");
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
        // PixelGenie L10: generate_audio_switch is the canonical `audio` (default off).
        assert!(
            matches!(schema.params.get("audio"), Some(ParamSpec::Bool(b)) if b.default == Some(false)),
            "{id} is missing audio (generate_audio_switch)"
        );
    }
}

/// PixVerse V6 Max (new 2026-11-03): "V6 MAX (v6-max) Only support
/// text-to-video/image-to-video now" — no first-and-last-frame transition —
/// with the capability map's "1-15 seconds", "360p / 540p / 720p / 1080p",
/// "16:9 / 9:16 / 4:3 / 3:4 / 1:1 / 2:3 / 3:2 / 21:9" and "optional native
/// audio"; no multi-clip switch ("V5.5 or V6"). Priced per second: "V6 Max |
/// Off | … 9 Credits/s USD 0.040/s" at 540p, so the default 5 s clip is $0.20.
/// @see <https://docs.platform.pixverse.ai/pixverse-api-pricing-2394995m0.md> (2026-11-03)
/// @see <https://docs.platform.pixverse.ai/model-capability-map-2394982m0.md> (2026-11-03)
#[test]
fn pixverse_v6_max_is_carried_with_its_documented_contract() {
    let id = "pixverse/v6-max";
    let reg = registry();
    let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
    assert!(schema.capabilities.text_to_video && schema.capabilities.image_to_video);
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(1), Some(15), Some(5))),
        other => panic!("{id} duration_seconds is {other:?}"),
    }
    let roles: Vec<&str> = schema.ref_inputs.as_ref().unwrap().roles.keys().map(String::as_str).collect();
    assert_eq!(roles, ["first_frame"], "no last frame on V6 Max");
    assert_eq!(aspect_ratios(&reg, id).len(), 8);
    assert!(matches!(schema.params.get("audio"), Some(ParamSpec::Bool(_))), "optional native audio");
    assert!(!schema.extra_allowlist.iter().any(|k| k == "generate_multi_clip_switch"), "multi-clip is V5.5 / V6 only");
    assert!((schema.pricing.base_cost_usd - 0.2).abs() < 1e-9);
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

// ─── bfl: vendor feature audit, 2026-11-03 ──────────────────────────────────

/// FLUX.1 [dev] (`POST /v1/flux-dev`): "steps … Number of steps for the image
/// generation process" (1–50, default 28) and "guidance … Guidance scale for
/// image generation" (1.5–5, default 3) are the canonical `steps` and
/// `guidance_scale`, not extras; "image_prompt … Optional base64 encoded image
/// to use as a prompt for generation" is a Redux reference.
/// @see <https://docs.bfl.ml/api-reference/models/generate-an-image-with-flux1-%5Bdev%5D> (2026-11-03)
#[test]
fn bfl_flux_dev_declares_steps_guidance_and_a_redux_reference() {
    let id = "bfl/flux-dev";
    let reg = registry();
    let schema = reg.get(id).expect("flux-dev missing");
    match (schema.params.get("steps"), schema.params.get("guidance_scale")) {
        (Some(ParamSpec::Int(s)), Some(ParamSpec::Float(g))) => {
            assert_eq!((s.min, s.max, s.default), (Some(1), Some(50), Some(28)), "steps 1-50, default 28");
            assert_eq!((g.min, g.max, g.default), (Some(1.5), Some(5.0), Some(3.0)), "guidance 1.5-5, default 3");
        }
        other => panic!("{id} steps / guidance_scale are {other:?}"),
    }
    for k in ["steps", "guidance"] {
        assert!(!schema.extra_allowlist.iter().any(|a| a == k), "{id}: {k} is a declared param, not an extra");
    }
    let refs = schema.ref_inputs.as_ref().expect("flux-dev declares no ref_inputs");
    assert_eq!(refs.roles.get("redux").map(|r| r.max_count), Some(1), "{id}: one image_prompt");
    assert!(schema.capabilities.image_to_image, "{id}: an image prompt is image-to-image");
}

/// FLUX 1.1 [pro] Ultra: "image_prompt … Optional image to remix in base64
/// format", with "image_prompt_strength … Blend between the prompt and the
/// image prompt" (0–1, default 0.1). The blend is the vendor's own extra, not
/// `strength`: there 1 keeps less of the image, here more.
/// @see <https://docs.bfl.ml/api-reference/models/generate-an-image-with-flux11-%5Bpro%5D-ultra-mode> (2026-11-03)
#[test]
fn bfl_flux_ultra_takes_an_image_prompt_to_remix() {
    let id = "bfl/flux-pro-1.1-ultra";
    let reg = registry();
    let schema = reg.get(id).expect("flux-pro-1.1-ultra missing");
    let refs = schema.ref_inputs.as_ref().expect("ultra declares no ref_inputs");
    assert_eq!(refs.roles.get("redux").map(|r| r.max_count), Some(1), "{id}: one image_prompt");
    assert!(schema.capabilities.image_to_image, "{id}: an image prompt is image-to-image");
    assert!(schema.extra_allowlist.iter().any(|a| a == "image_prompt_strength"), "{id}: the blend passes through");
}

/// FLUX.2: "Reference multiple images simultaneously - up to 8 via API".
/// [pro] and [max] take `input_image` … `input_image_8` ("Path to the eighth
/// input image"), [klein] 9B `input_image` … `input_image_4`. Each row took one.
/// @see <https://docs.bfl.ml/flux_2/flux2_image_editing.md> (2026-11-03)
/// @see <https://docs.bfl.ml/api-reference/models/generate-or-edit-an-image-with-flux2-%5Bpro%5D> (2026-11-03)
#[test]
fn bfl_flux_2_rows_take_every_input_image_the_api_does() {
    let reg = registry();
    for (id, n) in [("bfl/flux-2-pro", 8), ("bfl/flux-2-max", 8), ("bfl/flux-2-klein-9b", 4)] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        let refs = schema.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
        assert_eq!(refs.max_total, n, "{id}: {n} input images");
        assert_eq!(refs.roles.get("init").map(|r| r.max_count), Some(n), "{id}: the init role takes all {n}");
    }
}

// ─── ideogram: Ideogram 4.5, 2026-11-03 ──────────────────────────────────────

/// Ideogram 4.5 (`POST /v2/image/generate/ideogram-4-5`, multipart): prompt,
/// `images` ("max 5 … The first image is the one being edited"), `mask`,
/// `size` ("Without source images, an exact size must be one of the supported
/// 1K/2K presets"; with them "both sides a multiple of 32 … at most 2048x2048
/// pixels"), `background`, `quality` ("Defaults to `medium` with source
/// images and `high` without. `very_low` … requires source images"), `seed`.
/// Price (rendered): Generate Low / Medium / High $0.03 / $0.06 / $0.10, and
/// "with source images" Very Low / Low / Medium / High $0.008 / $0.03 /
/// $0.06 / $0.22, 1K and 2K alike.
/// @see <https://developer.ideogram.ai/v2/api-reference/images/generate/ideogram-4-5.md> (2026-11-03)
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (ResolutionV4, 2026-11-03)
/// @see <https://ideogram.ai/pricing/?pricing_tab=api> (rendered 2026-11-03)
#[test]
fn ideogram_4_5_is_carried_with_its_documented_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let id = "ideogram/ideogram-4-5";
    let m = reg.get(id).expect("ideogram/ideogram-4-5 missing");
    assert_eq!(m.docs_url.as_deref(), Some("https://developer.ideogram.ai/v2/api-reference/images/generate/ideogram-4-5"));
    assert!(m.capabilities.text_to_image && m.capabilities.image_to_image);
    assert!((m.pricing.base_cost_usd - 0.10).abs() < 1e-9, "text-to-image defaults to high");
    match m.params.get("quality") {
        Some(ParamSpec::String(q)) => {
            assert_eq!(q.enum_values, ["very_low", "low", "medium", "high"]);
            assert!(q.default.is_none(), "the vendor's default differs by mode, so none is declared");
        }
        other => panic!("{id} quality: {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "background"), ["transparent", "opaque"]);
    match m.params.get("size") {
        Some(ParamSpec::Size(SizeSpec::Enum(s))) => {
            assert_eq!(s.values.len(), 32, "ResolutionV4's presets with both sides a multiple of 32");
            assert!(s.values.contains(&(1024, 1024)) && s.values.contains(&(2048, 2048)) && s.values.contains(&(1440, 2880)));
            assert!(s.values.iter().all(|(w, h)| w % 32 == 0 && h % 32 == 0 && (*w as u64) * (*h as u64) <= 2048 * 2048));
        }
        other => panic!("{id} size: {other:?}"),
    }
    let refs = m.ref_inputs.as_ref().expect("refs");
    assert_eq!(refs.roles.get("init").map(|r| r.max_count), Some(1));
    assert_eq!(refs.roles.get("reference").map(|r| r.max_count), Some(4), "five images in all");
    assert!(refs.roles.contains_key("mask"));
    assert_eq!(m.extra_allowlist, ["magic_prompt", "enable_copyright_detection"]);
}

// ─── replicate: FLUX 3 Image, 2026-11-03 ─────────────────────────────────────

/// FLUX 3 Image on Replicate (`black-forest-labs/flux-3-image`, an official
/// model served versionless): `images` ("Up to 10 reference images"),
/// `resolution` 768sq … 4k (default 1k), 15 aspect ratios or auto,
/// `output_format` webp / jpg / png, `grounding`, `safety_tolerance` 0–4,
/// `output_quality`; no width/height, no seed. "$0.0205 per output image …
/// when target resolution equals 768sq", $0.024 at 1k, $0.035 at 1.5k, $0.05
/// at 2k, $0.3035 at 4k.
/// @see <https://replicate.com/black-forest-labs/flux-3-image/api/schema> (2026-11-03)
#[test]
fn replicate_flux_3_image_is_carried_with_its_documented_contract() {
    let reg = registry();
    let id = "replicate/flux-3-image";
    let m = reg.get(id).expect("replicate/flux-3-image missing");
    assert_eq!(m.docs_url.as_deref(), Some("https://replicate.com/black-forest-labs/flux-3-image/api/schema"));
    assert!(m.capabilities.text_to_image && m.capabilities.image_to_image);
    assert!((m.pricing.base_cost_usd - 0.024).abs() < 1e-9, "1k");
    for (tier, usd) in [("0.75K", 0.0205), ("1.5K", 0.035), ("2K", 0.05), ("4K", 0.3035)] {
        let rule = m.pricing.variable_pricing.iter().find(|r| r.when.get("resolution") == Some(&serde_json::json!(tier)));
        assert!(rule.and_then(|r| r.usd).is_some_and(|p| (p - usd).abs() < 1e-9), "{tier} is ${usd}");
    }
    assert_eq!(enum_values(&reg, id, "resolution"), ["0.75K", "1K", "1.5K", "2K", "4K"]);
    assert_eq!(aspect_ratios(&reg, id), ["21:9", "2:1", "16:9", "3:2", "7:5", "4:3", "5:4", "1:1", "4:5", "3:4", "5:7", "2:3", "9:16", "1:2", "9:21"]);
    assert_eq!(enum_values(&reg, id, "output_format"), ["png", "webp", "jpeg"]);
    for k in ["seed", "size", "steps", "guidance_scale"] {
        assert!(!m.params.contains_key(k), "{id} declares no {k}");
    }
    let refs = m.ref_inputs.as_ref().expect("refs");
    assert_eq!((refs.max_total, refs.roles.get("init").map(|r| r.max_count)), (10, Some(10)));
    for k in ["grounding", "safety_tolerance", "output_quality"] {
        assert!(m.extra_allowlist.iter().any(|a| a == k), "{k} passes through");
    }
}

// ─── vidu: Q4 Preview, 2026-11-03 ────────────────────────────────────────────

/// Vidu Q4 Preview ("Next-gen flagship for expressive audio & video"):
/// image-to-video (`images` "Only 1 image is accepted") and
/// reference-to-video ("supports uploading 1 to 15 images"); no text-to-video
/// or first-last frame page. Duration "3 - 16" (default 5); resolution
/// `540p` / `720p` / `1080p` / `2K` / `4K` (default 720p); aspect ratio on
/// reference-to-video `1:1` / `9:16` / `16:9` / `3:4` / `4:3`; `audio`
/// (default true) on both; prompts up to 20000 characters. Price: 9 / 19 /
/// 24 / 38 / 78 credits a second by resolution, at the $0.005 a credit every
/// Vidu row uses.
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q4-preview/image-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q4-preview/reference-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/pricing.md> (2026-11-03)
#[test]
fn vidu_q4_preview_is_carried_with_its_documented_contract() {
    let reg = registry();
    let id = "vidu/viduq4-preview";
    let m = reg.get(id).expect("vidu/viduq4-preview missing");
    assert!(m.capabilities.image_to_video && !m.capabilities.text_to_video, "image or references to video only");
    assert_eq!(m.docs_url.as_deref(), Some("https://platform.vidu.com/docs/api-reference/video-models/vidu-q4-preview/image-to-video"));
    assert_eq!(m.prompt.max_length, Some(20000));
    match m.params.get("duration_seconds") {
        Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(3), Some(16), Some(5))),
        other => panic!("{id} duration: {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "resolution"), ["540p", "720p", "1080p", "2K", "4K"]);
    let mut ratios = aspect_ratios(&reg, id);
    ratios.sort();
    assert_eq!(ratios, ["16:9", "1:1", "3:4", "4:3", "9:16"]);
    let refs = m.ref_inputs.as_ref().expect("refs");
    assert_eq!(refs.roles.get("first_frame").map(|r| r.max_count), Some(1));
    assert_eq!(refs.roles.get("reference").map(|r| r.max_count), Some(15));
    assert!(!refs.roles.contains_key("last_frame"), "no first-last frame");
    assert!(m.params.contains_key("audio"), "audio on both endpoints");
    assert!((m.pricing.base_cost_usd - 0.475).abs() < 1e-9, "720p, 5 s: 19 x 5 credits");
}

/// The Vidu models carried since 2026-11-03 (skipped 2026-09-11 as
/// "optional"), each on the endpoints its own operation documents and the
/// modes Vidu prices, at $0.005 a credit:
/// - viduq3: reference2video ("The viduq3 series models support uploading 1
///   to 7 images"), 3–16 s, 540p / 720p / 1080p, audio; 7 / 12 / 15 credits/s.
/// - viduq3-mix: reference2video (1 to 7 images), "available options: 3-15",
///   720p / 1080p, no audio switch; 24 / 29 credits/s.
/// - viduq3-pro-fast: img2video, 1–16 s, 720p / 1080p (start-end2video has
///   no price), audio; 12 / 15 credits/s.
/// - viduq2: text2video, 1–10 s, 540p / 720p / 1080p, audio; 10 / 15 / 20
///   credits the first second + 2 / 5 / 10 a second after.
/// - viduq2-turbo: img2video + start-end, 2–8 s (img2video's resolutions are
///   given "viduq2-turbo (2-8s)"), 540p / 720p / 1080p, audio on img2video.
/// - viduq1-classic: img2video + start-end, 5 s, 1080p; 80 credits.
///
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/reference-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q3/image-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/text-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q2/image-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/api-reference/video-models/vidu-q1/image-to-video.md> (2026-11-03)
/// @see <https://platform.vidu.com/docs/pricing.md> (2026-11-03)
#[test]
fn vidu_models_skipped_as_optional_are_carried_with_their_documented_contracts() {
    let reg = registry();
    for (id, t2v, roles, (dmin, dmax), res, prompt, audio, price) in [
        ("vidu/viduq3", false, &["reference"][..], (3, 16), &["540p", "720p", "1080p"][..], 5000, true, 0.30),
        ("vidu/viduq3-mix", false, &["reference"][..], (3, 15), &["720p", "1080p"][..], 5000, false, 0.60),
        ("vidu/viduq3-pro-fast", false, &["first_frame"][..], (1, 16), &["720p", "1080p"][..], 20000, true, 0.30),
        ("vidu/viduq2", true, &[][..], (1, 10), &["540p", "720p", "1080p"][..], 5000, true, 0.175),
        ("vidu/viduq2-turbo", false, &["first_frame", "last_frame"][..], (2, 8), &["540p", "720p", "1080p"][..], 2000, true, 0.24),
        ("vidu/viduq1-classic", false, &["first_frame", "last_frame"][..], (5, 5), &["1080p"][..], 2000, true, 0.40),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(m.docs_url.as_deref().is_some_and(|u| u.starts_with("https://platform.vidu.com/docs/api-reference/video-models/")), "{id}");
        assert_eq!((m.capabilities.text_to_video, m.capabilities.image_to_video), (t2v, !t2v), "{id} modes");
        let mut got: Vec<&str> = m.ref_inputs.as_ref().map(|r| r.roles.keys().map(String::as_str).collect()).unwrap_or_default();
        got.sort();
        let mut want = roles.to_vec();
        want.sort();
        assert_eq!(got, want, "{id} roles");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(dmin), Some(dmax), Some(5)), "{id} duration"),
            other => panic!("{id} duration: {other:?}"),
        }
        assert_eq!(enum_values(&reg, id, "resolution"), res, "{id}");
        assert_eq!(m.prompt.max_length, Some(prompt), "{id} prompt");
        assert_eq!(m.params.contains_key("audio"), audio, "{id} audio");
        assert!((m.pricing.base_cost_usd - price).abs() < 1e-9, "{id}: ${price}");
    }
}

// ─── bfl: FLUX.2 [flex], 2026-11-03 ──────────────────────────────────────────

/// FLUX.2 [flex] (`POST /v1/flux-2-flex`, `Flux2FlexInputs`): "Quality with
/// control", "Adjustable steps & guidance" — `steps` (1–50, default 50) and
/// `guidance` (1.5–10, default 5) — "Up to 8 (API)" references
/// (`input_image` … `input_image_8`), `prompt_upsampling`. Priced per
/// megapixel: the pricing page's firstMpPrice 5 and mpPrice 5 credits ($0.05
/// the first and each further megapixel).
/// @see <https://api.bfl.ai/openapi.json> (/v1/flux-2-flex, 2026-11-03)
/// @see <https://docs.bfl.ml/flux_2/flux2_overview.md> (2026-11-03)
/// @see <https://docs.bfl.ml/quick_start/pricing> (2026-11-03)
#[test]
fn bfl_flux_2_flex_is_carried_with_its_documented_contract() {
    let reg = registry();
    let m = reg.get("bfl/flux-2-flex").expect("bfl/flux-2-flex missing");
    assert_eq!(m.docs_url.as_deref(), Some("https://docs.bfl.ml/api-reference/models/generate-or-edit-an-image-with-flux2-%5Bflex%5D"));
    assert!(m.capabilities.text_to_image && m.capabilities.image_to_image);
    assert!((m.pricing.base_cost_usd - 0.05).abs() < 1e-9, "$0.05 for the first megapixel");
    match (m.params.get("steps"), m.params.get("guidance_scale")) {
        (Some(ParamSpec::Int(s)), Some(ParamSpec::Float(g))) => {
            assert_eq!((s.min, s.max, s.default), (Some(1), Some(50), Some(50)), "steps");
            assert_eq!((g.min, g.max, g.default), (Some(1.5), Some(10.0), Some(5.0)), "guidance");
        }
        other => panic!("steps / guidance_scale: {other:?}"),
    }
    let refs = m.ref_inputs.as_ref().expect("flex takes references");
    assert_eq!((refs.max_total, refs.roles.get("init").map(|r| r.max_count)), (8, Some(8)));
    assert!(m.extra_allowlist.iter().any(|k| k == "prompt_upsampling"), "prompt_upsampling passes through");
    assert_eq!(enum_values(&reg, "bfl/flux-2-flex", "output_format"), ["png", "webp", "jpeg"]);
}

// ─── bfl: FLUX 3 Image and FLUX 3 Image Klein, 2026-11-03 ───────────────────

/// FLUX 3 Image (`POST /v1/flux-3-image`, 2026-10-01) and FLUX 3 Image Klein
/// (`POST /v1/flux-3-image-klein`, 2026-10-15) generate and edit at one
/// endpoint each: "Same request as FLUX 3 Image: `prompt`, `images`,
/// `aspect_ratio`, `resolution` …". References are one `images` list, "one to
/// 10 total"; 15 aspect ratios (or `auto`, the default); `resolution` is the
/// price tier ("Per image, by `resolution`"; 1k when omitted). Both refuse a
/// field they lack (`additionalProperties: false`): FLUX 3 Image has no seed,
/// steps or output_format, and takes `grounding`; Klein adds "`seed`, `steps`,
/// and `output_format`" (jpeg | png) and its width/height "start at 64 pixels"
/// (up to 6256, 16,822,272 px). A custom size is "billed at the smallest
/// `resolution` with at least as many pixels. Anything above `2k` (about 4MP)
/// is billed as `4k`" (price ranges). Prices: the page's rates in credits,
/// "1 credit = $0.01 USD".
/// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
/// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3-klein> (2026-11-03)
/// @see <https://docs.bfl.ml/flux_3/flux3_image_klein.md> (2026-11-03)
/// @see <https://docs.bfl.ml/release-notes> (October 1, 15 and 22, 2026)
/// @see <https://docs.bfl.ml/quick_start/pricing> (2026-11-03)
#[test]
fn bfl_flux_3_image_and_klein_are_carried_with_their_documented_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let ratios = ["21:9", "2:1", "16:9", "3:2", "7:5", "4:3", "5:4", "1:1", "4:5", "3:4", "5:7", "2:3", "9:16", "1:2", "9:21"];
    // (id, its endpoint's API page, the 1k price, the other tiers' prices)
    for (id, page, base, tiers) in [
        ("bfl/flux-3-image", "generate-an-image-with-flux-3", 0.048, [("0.75K", 0.041), ("1.5K", 0.07), ("2K", 0.10), ("4K", 0.607)]),
        ("bfl/flux-3-image-klein", "generate-an-image-with-flux-3-klein", 0.009, [("0.75K", 0.008), ("1.5K", 0.011), ("2K", 0.014), ("4K", 0.036)]),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.docs_url.as_deref(), Some(format!("https://docs.bfl.ml/api-reference/utility/{page}").as_str()), "{id}");
        assert!(m.capabilities.text_to_image && m.capabilities.image_to_image, "{id}: generate or edit");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: the 1k default is ${base}");
        for (tier, usd) in tiers {
            let rule = m.pricing.variable_pricing.iter().find(|r| r.when.get("resolution") == Some(&serde_json::json!(tier)));
            assert!(rule.and_then(|r| r.usd).is_some_and(|p| (p - usd).abs() < 1e-9), "{id}: {tier} is ${usd}");
        }
        let refs = m.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
        assert_eq!((refs.max_total, refs.roles.get("init").map(|r| r.max_count)), (10, Some(10)), "{id}: one to 10 images");
        assert_eq!(aspect_ratios(&reg, id), ratios, "{id}");
        assert!(m.extra_allowlist.iter().any(|a| a == "safety_tolerance"), "{id}: safety_tolerance passes through");
        assert!(!m.params.contains_key("guidance_scale"), "{id}: no guidance");
    }
    let image = reg.get("bfl/flux-3-image").unwrap();
    for k in ["seed", "steps", "output_format"] {
        assert!(!image.params.contains_key(k), "FLUX 3 Image declares no {k}");
    }
    // "Exact output width in pixels, [256, 6256]"; "width x height at most
    // 16822272 pixels"; "at aspect ratios up to 8:1" (the 4 MP limit with
    // references is the adapter's to refuse).
    match image.params.get("size") {
        Some(ParamSpec::Size(SizeSpec::Freeform(s))) => {
            assert_eq!((s.min_width, s.max_width, s.min_height, s.max_height), (256, 6256, 256, 6256), "width/height");
            assert_eq!((s.max_pixels, s.max_aspect, s.multiple_of), (Some(16_822_272), Some(8.0), None), "16.8 MP, up to 8:1");
        }
        other => panic!("FLUX 3 Image size: {other:?}"),
    }
    assert!(image.extra_allowlist.iter().any(|a| a == "grounding"), "grounding passes through");
    let klein = reg.get("bfl/flux-3-image-klein").unwrap();
    assert!(!klein.extra_allowlist.iter().any(|a| a == "grounding"), "Klein has no grounding");
    match (klein.params.get("seed"), klein.params.get("steps"), klein.params.get("size")) {
        (Some(ParamSpec::Seed(seed)), Some(ParamSpec::Int(steps)), Some(ParamSpec::Size(SizeSpec::Freeform(s)))) => {
            assert_eq!((seed.min, seed.max), (0, 4_294_967_295), "seed");
            assert_eq!((steps.min, steps.max, steps.default), (Some(1), Some(16), Some(16)), "steps 1-16, default 16");
            assert_eq!((s.min_width, s.max_width, s.min_height, s.max_height), (64, 6256, 64, 6256), "width/height");
            assert_eq!((s.max_pixels, s.max_aspect, s.multiple_of), (Some(16_822_272), Some(8.0), None), "16.8 MP, up to 8:1");
        }
        other => panic!("Klein seed / steps / size: {other:?}"),
    }
    assert_eq!(enum_values(&reg, "bfl/flux-3-image-klein", "output_format"), ["png", "jpeg"], "jpeg | png, no webp");
    // Bounding boxes: "Write a description using `<id>` to name each element,
    // then append a JSON array of rows" — litegen's `layout`, object and
    // text boxes, on both rows (text-to-image, as litegen's layout is).
    for id in ["bfl/flux-3-image", "bfl/flux-3-image-klein"] {
        match reg.get(id).unwrap().params.get("layout") {
            Some(ParamSpec::Layout(l)) => {
                assert_eq!(l.kinds, [litegen::capabilities::LayoutKind::Object, litegen::capabilities::LayoutKind::Text], "{id}");
                assert!(!l.single_word_text, "{id}: a text box may hold a line");
            }
            other => panic!("{id} layout: {other:?}"),
        }
    }
}

// ─── stability: vendor feature audit, 2026-11-03 ────────────────────────────

const STABILITY_STYLE_PRESETS: [&str; 17] = [
    "3d-model", "analog-film", "anime", "cinematic", "comic-book", "digital-art", "enhance", "fantasy-art",
    "isometric", "line-art", "low-poly", "modeling-compound", "neon-punk", "origami", "photographic", "pixel-art",
    "tile-texture",
];

/// Every route that takes `style_preset` ("StylePreset … Guides the image model
/// towards a particular style": the same 17 values on /generate/sd3, /ultra,
/// /core, /edit/inpaint, /outpaint, /search-and-replace and the v1 SDXL
/// engine) declares it as the canonical `style`, never as an extra. Only Core
/// declared it.
/// @see <https://docs.stability.ai/openapi.json> (2026-11-03)
/// @see <https://docs.stability.ai/openapi-v1.json> (2026-11-03)
#[test]
fn stability_style_presets_are_the_canonical_style_wherever_a_route_takes_them() {
    let reg = registry();
    for id in [
        "stability/sd3-large", "stability/sd3.5-medium", "stability/sd3-turbo", "stability/core", "stability/ultra",
        "stability/sdxl", "stability/inpaint", "stability/outpaint", "stability/search-and-replace",
    ] {
        let mut got = enum_values(&reg, id, "style");
        got.sort();
        assert_eq!(got, STABILITY_STYLE_PRESETS, "{id}: style_preset's 17 values");
        let schema = reg.get(id).unwrap();
        assert!(!schema.extra_allowlist.iter().any(|k| k == "style_preset"), "{id}: style_preset is `style`, not an extra");
    }
    assert!(enum_values(&reg, "stability/remove-background", "style").is_empty(), "remove-background takes no style_preset");
}

/// /generate/sd3: "cfg_scale … How strictly the diffusion process adheres to
/// the prompt text" (1–10): "The _Large_ and _Medium_ models use a default of
/// `4`. The _Turbo_ and _Flash_ model uses a default of `1`." It is the
/// canonical `guidance_scale`; no sd3 row declared it.
/// @see <https://docs.stability.ai/openapi.json> (2026-11-03)
#[test]
fn stability_sd3_rows_take_cfg_scale_as_guidance() {
    let reg = registry();
    for (id, default) in [("stability/sd3-large", 4.0), ("stability/sd3.5-medium", 4.0), ("stability/sd3-turbo", 1.0)] {
        match reg.get(id).unwrap_or_else(|| panic!("{id} missing")).params.get("guidance_scale") {
            Some(ParamSpec::Float(g)) => assert_eq!((g.min, g.max, g.default), (Some(1.0), Some(10.0), Some(default)), "{id}"),
            other => panic!("{id} guidance_scale is {other:?}"),
        }
    }
}

/// /generate/ultra: "image … The image to use as the starting point for the
/// generation", with "strength … required when `image` is provided" (0–1).
/// /generate/sd3 takes `image` + `strength` in `mode: image-to-image` for
/// every model in its enum, sd3.5-large-turbo included. Neither row took one.
/// @see <https://docs.stability.ai/openapi.json> (2026-11-03)
#[test]
fn stability_ultra_and_sd3_turbo_take_an_image_to_start_from() {
    let reg = registry();
    for id in ["stability/ultra", "stability/sd3-turbo"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(schema.capabilities.image_to_image, "{id}: image-to-image");
        let refs = schema.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
        assert_eq!(refs.roles.get("init").map(|r| (r.required, r.max_count)), Some((false, 1)), "{id}: one optional starting image");
        match &refs.provider_format {
            litegen::capabilities::schema::RefProviderFormat::Multipart(m) => {
                assert_eq!(m.field_map.get("init").map(String::as_str), Some("image"), "{id}: sent as `image`")
            }
            other => panic!("{id} refs go as {other:?}"),
        }
        match schema.params.get("strength") {
            Some(ParamSpec::Float(s)) => assert_eq!((s.min, s.max), (Some(0.0), Some(1.0)), "{id}"),
            other => panic!("{id} strength is {other:?}"),
        }
    }
}

/// /edit/inpaint's `prompt` and `negative_prompt` are the shared Prompt and
/// NegativePrompt schemas ("maxLength": 10000), and it takes "output_format …
/// jpeg | png | webp" (default png; "Dictates the `content-type` of the
/// generated image"). The row had neither limit and no output_format.
/// @see <https://docs.stability.ai/openapi.json> (2026-11-03)
#[test]
fn stability_inpaint_carries_its_prompt_limits_and_output_format() {
    let reg = registry();
    let schema = reg.get("stability/inpaint").expect("inpaint missing");
    assert_eq!(schema.prompt.max_length, Some(10000), "Prompt maxLength 10000");
    match schema.params.get("negative_prompt") {
        Some(ParamSpec::String(s)) => assert_eq!(s.max_length, Some(10000), "NegativePrompt maxLength 10000"),
        other => panic!("inpaint negative_prompt is {other:?}"),
    }
    let mut formats = enum_values(&reg, "stability/inpaint", "output_format");
    formats.sort();
    assert_eq!(formats, ["jpeg", "png", "webp"]);
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
            !schema.params.contains_key(param),
            "{id} declares `{param}`, which black-forest-labs/flux-1.1-pro does not take"
        );
    }
}

// ─── fal: 2026-09-11 drift decisions applied ────────────────────────────────

/// `fal-ai/flux-2` (FLUX.2 [dev]) input: `guidance_scale` 0-20 (default
/// 2.5), `num_inference_steps` 4-50 (default 28), and `image_size` width and
/// height 512-2048. Since 2026-11-04 images go to its /edit twin,
/// `fal-ai/flux-2/edit` (`image_urls`, "A maximum of 4 images").
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2> (2026-09-11)
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux-2/edit> (2026-11-04)
#[test]
fn fal_flux_2_matches_the_endpoint_input_schema() {
    let id = "fal/flux-2";
    let reg = registry();
    let schema = reg
        .get(id)
        .unwrap_or_else(|| panic!("{id} missing from the catalog"));
    assert!(
        schema.capabilities.text_to_image && schema.capabilities.image_to_image,
        "{id}: text at fal-ai/flux-2, images at fal-ai/flux-2/edit"
    );
    assert_eq!(schema.ref_inputs.as_ref().map(|r| r.max_total), Some(4), "{id}: \"A maximum of 4 images\"");
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
    assert_eq!(enum_values(&reg, "kling/kling-v3", "resolution"), ["1K", "2K"], "resolution selects 1k/2k");
    for key in ["image_reference", "image_fidelity", "human_fidelity"] {
        assert!(
            !schema.extra_allowlist.iter().any(|k| k == key),
            "kling/kling-v3 allowlists {key}, which Image 3.0 does not support"
        );
    }
}

/// Two new Kling video models on the current design (2026-11-03):
/// - Kling 3.0 Turbo: "`settings.duration` | int | … `3` … `15`",
///   "`settings.resolution` … `720p`, `1080p`", "`settings.aspect_ratio` …
///   `16:9`, `9:16`, `1:1`"; image-to-video "`contents[].type` … `prompt`,
///   `first_frame`" (no last frame); its prompt item "cannot exceed 2500
///   characters"; no `settings.audio` (priced only "With Native Audio").
/// - Kling 4.0: duration `3` … `30` (default 10), resolution `720p`, `1080p`
///   (default 1080p), aspect ratio `21:9`, `16:9`, `9:16`, `1:1`, frames
///   first / middle / last (the middle frame is not carried), a prompt of "up
///   to 11,000 characters" for non-English input; no `settings.audio`.
///
/// @see <https://kling.ai/document-api/api/video/3-0-turbo/text-to-video.md> (2026-11-03)
/// @see <https://kling.ai/document-api/api/video/4-0/text-to-video.md> (2026-11-03)
/// @see <https://kling.ai/document-api/pricing/base/video.md> (2026-11-03)
#[test]
fn kling_3_0_turbo_and_4_0_are_carried_with_their_documented_contract() {
    let reg = registry();
    for (id, (min, max, default), res, res_default, ratios, roles, prompt, price) in [
        ("kling/video-kling-v3-turbo", (3, 15, 5), &["1080p", "720p"][..], "720p", &["16:9", "1:1", "9:16"][..], &["first_frame"][..], 2500, 0.56),
        ("kling/video-kling-v4", (3, 30, 10), &["1080p", "720p"][..], "1080p", &["16:9", "1:1", "21:9", "9:16"][..], &["first_frame", "last_frame"][..], 11000, 2.35),
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(schema.capabilities.text_to_video && schema.capabilities.image_to_video, "{id}");
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(min), Some(max), Some(default)), "{id}"),
            other => panic!("{id} duration_seconds is {other:?}"),
        }
        let mut got = enum_values(&reg, id, "resolution");
        got.sort();
        assert_eq!(got, res, "{id}");
        match schema.params.get("resolution") {
            Some(ParamSpec::String(r)) => assert_eq!(r.default.as_deref(), Some(res_default), "{id}: the vendor's default"),
            other => panic!("{id} resolution is {other:?}"),
        }
        let mut got = aspect_ratios(&reg, id);
        got.sort();
        assert_eq!(got, ratios, "{id}");
        let mut got: Vec<&str> = schema.ref_inputs.as_ref().unwrap().roles.keys().map(String::as_str).collect();
        got.sort();
        assert_eq!(got, roles, "{id}");
        assert_eq!(schema.prompt.max_length, Some(prompt), "{id}");
        assert!(!schema.params.contains_key("audio"), "{id}: no settings.audio switch");
        assert!(!schema.params.contains_key("negative_prompt"), "{id}");
        assert!((schema.pricing.base_cost_usd - price).abs() < 1e-9, "{id}: the default request's price");
    }
}

/// Kling's current API design (2026-11-03): the text-to-video body is
/// `prompt` ("The prompt can include positive and negative descriptions"),
/// `settings` and `options`, and image-to-video's is `contents`, `settings`
/// and `options` — no negative prompt anywhere, so no video row declares one;
/// Kling 3.0's only allowlisted extra is `settings.multi_shot` (multi-shot is
/// written in the prompt, "shot n, m, words;", so the legacy `shot_type` and
/// `multi_prompt` are gone).
/// @see <https://kling.ai/document-api/api/video/2-6/text-to-video.md> (2026-11-03)
/// @see <https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md> (2026-11-03)
#[test]
fn kling_video_rows_match_the_current_api_design() {
    let reg = registry();
    for id in ["kling/video-kling-v2-5-turbo", "kling/video-kling-v2-6", "kling/video-kling-v3"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(!schema.params.contains_key("negative_prompt"), "{id}: the current body has no negative prompt");
        let want: &[&str] = if id.ends_with("v3") { &["multi_shot"] } else { &[] };
        assert_eq!(schema.extra_allowlist, want, "{id}");
    }
}

/// Kling 3.0 on the legacy text2video / image2video endpoints (both
/// `model_name` enums list kling-v3): text and image input, 3-15 s,
/// 16:9 | 9:16 | 1:1, mode std | pro | 4k, native audio (`sound`),
/// first + last frame. Camera control is "Not Supported" and cfg_scale is not
/// documented for 3.0, so neither is allowlisted.
/// @see https://kling.ai/document-api/api/video/3-0-omni/text-to-video.md (2026-10-30):
///   "`settings.duration` … `3`, `4`, … `15`", "`settings.aspect_ratio` …
///   `16:9`, `9:16`, `1:1`" (the camera quote is from the legacy video/1-6 page,
///   read 2026-09-11 and since unpublished)
/// @see https://kling.ai/document-api/guides/capability-map/video.md (2026-10-30)
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
    assert!(matches!(schema.params.get("audio"), Some(ParamSpec::Bool(_))), "{id}: Kling 3.0 documents sound (canonical audio)");
    // "Kling 3.0 | … | 720P, 1080P, 4K" is the canonical `resolution`, sent as
    // the legacy `mode` (std | pro | 4k), never an extra (2026-11-03).
    let mut res = enum_values(&reg, id, "resolution");
    res.sort();
    assert_eq!(res, ["1080p", "4k", "720p"], "{id}: 720p, 1080p or 4k");
    assert!(!schema.extra_allowlist.iter().any(|k| k == "mode"), "{id}: mode is derived from resolution");
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
    // "duration {"type":"integer","enum":[4,6,8],"default":8}" (Leonardo's v2
    // model list, 2026-11-03): 5 and 7 s were advertised and then refused.
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(f)) => {
            assert_eq!(f.values, vec![4.0, 6.0, 8.0], "Veo 3.1 Fast: 4, 6 or 8 s, nothing between");
            assert_eq!(f.default, Some(8.0), "Veo 3.1 Fast defaults to 8 s");
        }
        other => panic!("veo3.1-fast duration_seconds is {other:?}"),
    }
}

// ─── ideogram: vendor feature audit, 2026-11-03 ─────────────────────────────

/// Ideogram 3.0 (`POST /v1/ideogram-v3/generate`): "resolution … The
/// resolutions supported for Ideogram 3.0" (69 WxH values) is the canonical
/// `size`, and "aspect_ratio … Cannot be combined with resolution". No 3.0
/// row declared a size, so the adapter's `resolution` mapping was unreachable.
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v1.json> (2026-11-03)
#[test]
fn ideogram_v3_rows_take_the_documented_resolutions_as_size() {
    use litegen::capabilities::schema::SizeSpec;
    const RESOLUTION_V3: &str = "512x1536 576x1408 576x1472 576x1536 640x1344 640x1408 640x1472 640x1536 \
        704x1152 704x1216 704x1280 704x1344 704x1408 704x1472 736x1312 768x1088 768x1216 768x1280 768x1344 \
        800x1280 832x960 832x1024 832x1088 832x1152 832x1216 832x1248 864x1152 896x960 896x1024 896x1088 \
        896x1120 896x1152 960x832 960x896 960x1024 960x1088 1024x832 1024x896 1024x960 1024x1024 1088x768 \
        1088x832 1088x896 1088x960 1120x896 1152x704 1152x832 1152x864 1152x896 1216x704 1216x768 1216x832 \
        1248x832 1280x704 1280x768 1280x800 1312x736 1344x640 1344x704 1344x768 1408x576 1408x640 1408x704 \
        1472x576 1472x640 1472x704 1536x512 1536x576 1536x640";
    let mut want: Vec<String> = RESOLUTION_V3.split_whitespace().map(str::to_string).collect();
    want.sort();
    assert_eq!(want.len(), 69);
    let reg = registry();
    for id in ["ideogram/ideogram-v3", "ideogram/ideogram-v3-turbo", "ideogram/ideogram-v3-quality"] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        match schema.params.get("size") {
            Some(ParamSpec::Size(SizeSpec::Enum(e))) => {
                let mut got: Vec<String> = e.values.iter().map(|(w, h)| format!("{w}x{h}")).collect();
                got.sort();
                assert_eq!(got, want, "{id}: the ResolutionV3 enum");
            }
            other => panic!("{id} size is {other:?}, expected the 3.0 resolution enum"),
        }
    }
}

/// 4.5 Precise Edit: "`reference_images` (files, optional) — Optional images
/// to guide the edit (max 4 …). They are never edited themselves … A request
/// with a `mask` can include at most three, because the mask takes up one
/// reference slot." The row took none. Image + mask + references ≤ 5 holds the
/// slot rule for a mask the caller sends (a drawn one is the adapter's check).
/// @see <https://developer.ideogram.ai/v2/api-reference/images/precise-edit/ideogram-4-5.md> (2026-11-03)
#[test]
fn ideogram_precise_edit_takes_up_to_four_guiding_references() {
    let reg = registry();
    let m = reg.get("ideogram/ideogram-4-5-precise-edit").expect("the row");
    let ri = m.ref_inputs.as_ref().expect("precise edit declares no ref_inputs");
    assert_eq!(ri.roles.get("reference").map(|r| (r.required, r.max_count)), Some((false, 4)), "up to 4 guiding images");
    assert_eq!(ri.max_total, 5, "the image, and a mask and the references sharing 4 slots");
    match &ri.provider_format {
        litegen::capabilities::schema::RefProviderFormat::Multipart(mp) => {
            assert_eq!(mp.field_map.get("reference").map(String::as_str), Some("reference_images"))
        }
        other => panic!("precise edit refs go as {other:?}"),
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

/// MiniMax-H3-Max: "Supports text-to-video, image-to-video (first / last
/// frame), and reference-to-video (reference image / video / audio); `480P`
/// / `768P` resolution, **`2K` is not supported**; 5–15s duration."
/// @see <https://platform.minimax.io/docs/api-reference/video-generation-v2-create.md> (2026-10-30)
#[test]
fn minimax_h3_max_has_no_2k_no_4s_and_takes_references() {
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
    assert_eq!(roles.get("reference").map(|r| r.max_count), Some(9), "reference images ≤ 9, as on H3");
}

// ─── hunyuan: 2026-09-11 drift decisions applied ────────────────────────────

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

/// 3D doc §2 rule 1: every model's `task` belongs to its modality, and the
/// `*_to_3d` flags appear only on 3D `generate` models.
#[test]
fn every_model_declares_a_task_its_modality_has() {
    use litegen::capabilities::schema::{MediaType, ModelTask};
    let reg = registry();
    for m in reg.all() {
        assert!(m.task.allowed_for(m.media_type), "{} declares task {:?} on a {:?} model", m.id, m.task, m.media_type);
        let c = &m.capabilities;
        if c.text_to_3d || c.image_to_3d || c.multiview_to_3d {
            assert_eq!(m.media_type, MediaType::Model3d, "{} sets a *_to_3d flag", m.id);
            assert_eq!(m.task, ModelTask::Generate, "{} sets a *_to_3d flag on a {:?} model", m.id, m.task);
        }
    }
}

/// 3D doc §3A: every 3D operation says which sources its vendor takes.
#[test]
fn every_3d_operation_declares_the_source_its_vendor_takes() {
    use litegen::capabilities::ModelTask;
    let reg = registry();
    for m in model3d_models(&reg).into_iter().filter(|m| m.task != ModelTask::Generate) {
        match m.params.get("source_model") {
            Some(ParamSpec::SourceModel(s)) => assert!(!s.formats.is_empty(), "{} takes no source format", m.id),
            other => panic!("{} is a {:?} operation without a source_model spec: {other:?}", m.id, m.task),
        }
    }
}

// ─── Partial image edits (PixelGenie 3D doc §4) ─────────────────────────────

#[test]
fn the_inpainting_flag_means_the_model_takes_a_mask() {
    // `supports_inpainting` is "takes a mask"; a flag without the role (or the
    // reverse) would advertise what the validator refuses.
    let reg = registry();
    for m in reg.all() {
        let takes_mask = m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("mask"));
        assert_eq!(m.capabilities.inpainting, takes_mask, "{}: inpainting flag vs mask role", m.id);
    }
}

/// @see <https://developer.ideogram.ai/v2/api-reference/images/precise-edit/ideogram-4-5.md>
/// (2026-10-30): "Black marks the area to edit"; context_window "at least
/// 256px on each side … between 1:6 and 6:1"; pixels "copied exactly";
/// quality very_low | low | medium | high. Price (rendered pricing page):
/// $0.008 / $0.03 / $0.06 / $0.22, medium the default.
#[test]
fn ideogram_precise_edit_takes_a_black_mask_and_a_256_px_box() {
    use litegen::capabilities::{MaskConvention, ParamSpec, PreservesUnmasked, RegionInput};
    let reg = registry();
    let m = reg.get("ideogram/ideogram-4-5-precise-edit").expect("the row");
    let mask = &m.ref_inputs.as_ref().unwrap().roles["mask"];
    assert_eq!((mask.convention, mask.native_grow), (Some(MaskConvention::BlackEdits), false));
    match m.params.get("region") {
        Some(ParamSpec::Region(r)) => assert_eq!((r.input, r.min_window_px, r.max_aspect), (RegionInput::Box, Some(256), Some(6.0))),
        other => panic!("{other:?}"),
    }
    assert_eq!(m.preserves_unmasked, Some(PreservesUnmasked::Exact));
    assert!((m.pricing.base_cost_usd - 0.06).abs() < 1e-9);
    assert!(m.docs_url.as_deref().is_some_and(|u| u.contains("precise-edit/ideogram-4-5")));
}

/// @see <https://api.stability.ai/v2alpha/openapi> (2026-10-30): POST
/// /v2beta/stable-image/edit/inpaint — mask white is full strength; grow_mask
/// default 5. Price: "Inpaint … 5" credits, "1 credit equals $0.01".
#[test]
fn stability_inpaint_takes_a_white_mask_and_grows_it_itself() {
    use litegen::capabilities::MaskConvention;
    let reg = registry();
    let m = reg.get("stability/inpaint").expect("the row");
    let mask = &m.ref_inputs.as_ref().unwrap().roles["mask"];
    assert_eq!((mask.convention, mask.native_grow), (Some(MaskConvention::WhiteEdits), true));
    assert!((m.pricing.base_cost_usd - 0.05).abs() < 1e-9);
    // OpenAI reads transparency, the other way round from a white mask.
    for id in ["openai/gpt-image-2", "openai/gpt-image-2.5-sunburst", "openai/gpt-image-2.5-flare"] {
        let o = reg.get(id).unwrap_or_else(|| panic!("{id} is a carried row"));
        assert_eq!(o.ref_inputs.as_ref().unwrap().roles["mask"].convention, Some(MaskConvention::TransparentEdits), "{id}");
    }
}

// ─── Multiview (PixelGenie 3D doc §3E) ──────────────────────────────────────

/// @see <https://developers.tripo3d.ai/en/docs/generation-image-to-multiview>
/// (2026-10-30): one field, `input`; the result "contains URLs for available
/// views, such as front_view_url, left_view_url, back_view_url and
/// right_view_url". Price: "Image to Multiview" 10 credits, "1 credit = $0.01
/// USD".
#[test]
fn tripo_multiview_offers_the_four_views_tripo_returns_from_one_image() {
    use litegen::capabilities::{ModelTask, ParamSpec};
    let reg = registry();
    let m = reg.get("tripo/multiview").expect("the row");
    assert_eq!(m.task, ModelTask::Multiview);
    match m.params.get("views") {
        Some(ParamSpec::StringArray(v)) => {
            assert_eq!(v.enum_values, ["front", "left", "back", "right"]);
            assert_eq!(v.default, ["front", "left", "back", "right"]);
        }
        other => panic!("{other:?}"),
    }
    let refs = m.ref_inputs.as_ref().unwrap();
    assert_eq!((refs.max_total, refs.roles["init"].required), (1, true));
    assert_eq!(m.prompt.max_length, Some(0), "the endpoint has no prompt field");
    assert!((m.pricing.base_cost_usd - 0.10).abs() < 1e-9);
}

/// Every multiview-to-3D vendor needs the front view, and one that reads its
/// views by position declares that order: fal's Tripo H3.1 takes `image_urls`
/// "2 to 4 … Order: [front, left, back, right]. Front view is required."
/// (<https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=tripo3d/h3.1/multiview-to-3d>,
/// 2026-10-30).
#[test]
fn every_multiview_to_3d_model_takes_a_front_view_and_names_any_positional_order() {
    let reg = registry();
    for m in model3d_models(&reg).into_iter().filter(|m| m.capabilities.multiview_to_3d) {
        let refs = m.ref_inputs.as_ref().unwrap();
        assert!(refs.roles.contains_key("view-front"), "{}: every vendor needs the front", m.id);
        if let Some(first) = refs.view_order.first() {
            assert_eq!(first, "front", "{}", m.id);
        }
    }
    let tripo = reg.get("fal/tripo-h3.1-multiview").expect("the row");
    let refs = tripo.ref_inputs.as_ref().unwrap();
    assert_eq!(refs.view_order, ["front", "left", "back", "right"]);
    assert!(refs.roles["view-front"].required && refs.roles["view-left"].required, "two images at least, read as front and left");
    assert!(!refs.roles.contains_key("init"), "multiview only");
}

// ─── Request-time checks (PixelGenie L13) ──────────────────────────────────

/// @see <https://developers.openai.com/api/docs/guides/image-generation>
/// ("GPT Image 2 settings"): both edges divisible by 16, neither above 3840,
/// 655,360–8,294,400 total pixels, and a long-to-short ratio of at most 3:1.
/// The size spec carries every limit, so a size OpenAI cannot make is
/// refused before billing instead of being snapped.
#[test]
fn gpt_image_sizes_carry_every_openai_limit() {
    use litegen::capabilities::{ParamSpec, SizeSpec};
    let reg = registry();
    let rows: Vec<_> = reg.all().filter(|m| m.id.starts_with("openai/gpt-image-2")).collect();
    assert!(!rows.is_empty());
    for m in rows {
        match m.params.get("size") {
            Some(ParamSpec::Size(SizeSpec::Freeform(f))) => assert_eq!(
                (f.multiple_of, f.max_width, f.max_height, f.min_pixels, f.max_pixels, f.max_aspect),
                (Some(16), 3840, 3840, Some(655_360), Some(8_294_400), Some(3.0)),
                "{}",
                m.id
            ),
            other => panic!("{}: {other:?}", m.id),
        }
    }
}

// ─── Prices that match the bill (PixelGenie 3D doc §3G, L14) ────────────────

/// A row's base price is what its own rules charge for its default request —
/// every param at its default, one clip on an animate model, keyframes 0 s and
/// 5 s on a keyframe model — so the listing's `base_cost_usd` stays the
/// vendor's price for the request a caller gets by sending nothing extra.
#[test]
fn every_base_price_is_what_the_rules_charge_for_the_default_request() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    let mut priced = 0;
    for m in reg.all().filter(|m| !m.pricing.variable_pricing.is_empty()) {
        priced += 1;
        let mut req = serde_json::json!({ "model": m.id, "prompt": "x" });
        if m.params.contains_key("animations") {
            req["animations"] = serde_json::json!([{ "name": "a" }]);
        }
        if m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("keyframe")) {
            req["reference_images"] = serde_json::json!([
                { "type": "url", "value": "a", "role": "keyframe", "time_s": 0.0 },
                { "type": "url", "value": "b", "role": "keyframe", "time_s": 5.0 },
            ]);
        }
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        let price = unit_price_usd(m, &PriceFacts::new(m, &base, &req));
        assert!(
            (price - m.pricing.base_cost_usd).abs() < 1e-6,
            "{}: base_cost_usd {} but its rules charge {price} for the default request",
            m.id,
            m.pricing.base_cost_usd
        );
    }
    assert!(priced > 0);
}

// ─── Timed keyframes (PixelGenie 3D doc §5) ────────────────────────────────

/// @see <https://docs.platform.pixverse.ai/multi-transition-video-43051215e0.md>
/// (2026-10-30): multi_transition "minItems: 2" of { img_id, duration
/// (integer, minimum 0), prompt }; model v3.5 | v4 | v4.5 | v5, and the
/// capability map supports it on V5 alone. Price: 5 s at 360p/540p "45
/// Credits", US$0.200 ("total Credits ÷ 225").
#[test]
fn pixverse_multi_transition_takes_two_or_more_whole_second_keyframes() {
    use litegen::capabilities::KeyframeTiming;
    let reg = registry();
    let m = reg.get("pixverse/v5-multi-transition").expect("the row");
    let refs = m.ref_inputs.as_ref().unwrap();
    let k = &refs.roles["keyframe"];
    // No maxItems is documented; 31 at whole seconds span the longest priced clip (30 s).
    assert_eq!((k.required, k.min_count, k.max_count), (true, 2, 31));
    assert_eq!((k.timing, k.time_step_s), (Some(KeyframeTiming::Any), Some(1.0)), "integer durations");
    assert_eq!(refs.roles.len(), 1, "keyframes only");
    assert!(!m.params.contains_key("duration_seconds"), "the keyframes make the clip");
    assert!((m.pricing.base_cost_usd - 0.20).abs() < 1e-9);
}

// ─── References for URL-only vendors (PixelGenie L4) ────────────────────────

/// @see <https://docs.lumalabs.ai/docs/image-generation.md> (2026-10-30): "You
/// should upload and use your own cdn image urls, currently this is the only
/// way to pass an image". fal documents data: URIs as file inputs
/// (<https://fal.ai/docs/documentation/model-apis/fal-cdn>), Runway takes them
/// "in any field where a URL is accepted", Vidu and Replicate take them too.
#[test]
fn only_vendors_that_refuse_data_uris_get_published_references() {
    let reg = registry();
    for m in reg.all() {
        let Some(refs) = &m.ref_inputs else { continue };
        let refuses = m.provider == "luma" || m.id == "mock/url-only-image";
        assert_eq!(!refs.data_uri, refuses, "{}: data_uri", m.id);
    }
}

// ─── Segmentation (PixelGenie L7) ───────────────────────────────────────────

/// fal's SAM 3 and SAM 3.1 take a text `prompt`, `point_prompts` and
/// `box_prompts`, and return at most `max_masks` "1 to 32" masks
/// (<https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/sam-3/image>,
/// `…=fal-ai/sam-3-1/image`, 2026-10-30). Price: "$0.005 per request" and
/// "$0.01 per request" (`/llms.txt`, 2026-10-30).
#[test]
fn fal_sam_rows_take_all_three_prompt_types_and_at_most_32_masks() {
    use litegen::capabilities::{MediaType, ModelTask, ParamSpec, SegmentationPromptType as T};
    let reg = registry();
    for (id, price) in [("fal/sam-3", 0.005), ("fal/sam-3.1", 0.01)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!((m.media_type, m.task), (MediaType::Image, ModelTask::Segment), "{id}");
        match m.params.get("prompts") {
            Some(ParamSpec::SegmentationPrompts(p)) => {
                assert_eq!(p.types, [T::Point, T::Box, T::Text], "{id}");
                assert_eq!(p.max_masks, 32, "{id}");
            }
            other => panic!("{id}: {other:?}"),
        }
        assert!((m.pricing.base_cost_usd - price).abs() < 1e-9, "{id}");
        assert!(m.pricing.variable_pricing.is_empty(), "{id}: one price per request");
        assert!(m.docs_url.as_deref().is_some_and(|u| u.contains(&id.replace("fal/", "fal-ai/").replace("3.1", "3-1"))), "{id}");
    }
}

// ─── Background removal (PixelGenie L8) ─────────────────────────────────────

/// Each matting row restates its vendor's price and container choice:
/// Bria RMBG 2.0 "$0.018 per image without a prompt" and BiRefNet v2 "$0 per
/// compute seconds" (fal `llms.txt`, 2026-10-30; output_format webp | png |
/// gif); Recraft "Remove background … $0.01", image_format webp | png
/// (endpoints.md, remove-background.md); Stability "Remove Background … 5"
/// credits at "$0.01" each, output_format png | webp (pricing.md, v2alpha
/// openapi).
#[test]
fn background_removal_rows_restate_their_vendors_prices_and_containers() {
    use litegen::capabilities::ParamSpec;
    let reg = registry();
    for (id, price, formats) in [
        ("fal/bria-rmbg-2", 0.018, None),
        ("fal/birefnet-v2", 0.0, Some(["png", "webp"])),
        ("recraft/remove-background", 0.01, Some(["png", "webp"])),
        ("stability/remove-background", 0.05, Some(["png", "webp"])),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id}"));
        assert!(m.capabilities.background_removal, "{id}");
        assert!((m.pricing.base_cost_usd - price).abs() < 1e-9, "{id}");
        let offered = match m.params.get("output_format") {
            Some(ParamSpec::String(f)) => Some(f.enum_values.clone()),
            _ => None,
        };
        assert_eq!(offered, formats.map(|f| f.map(String::from).to_vec()), "{id}");
        assert!(m.docs_url.is_some(), "{id}");
    }
    // Bria's prompt is billed $0.03 instead of $0.018, so the row takes none.
    assert_eq!(reg.get("fal/bria-rmbg-2").unwrap().prompt.max_length, Some(0));
}

// ─── Expanded canvases (PixelGenie 3D doc §4, P2) ───────────────────────────

/// Stability's Outpaint takes `left`, `right`, `up` and `down`, each "minimum":
/// 0, "maximum": 2000 (<https://api.stability.ai/v2alpha/openapi>,
/// 2026-10-30); "Outpaint … 4" credits at "$0.01" each (pricing.md).
#[test]
fn stability_outpaint_grows_each_side_by_up_to_2000_px() {
    use litegen::capabilities::ParamSpec;
    let reg = registry();
    let m = reg.get("stability/outpaint").expect("the row");
    match m.params.get("expand") {
        Some(ParamSpec::Expand(e)) => assert_eq!((e.centered_only, e.max_px), (false, Some(2000))),
        other => panic!("{other:?}"),
    }
    assert!(m.ref_inputs.as_ref().unwrap().roles["init"].required);
    assert!((m.pricing.base_cost_usd - 0.04).abs() < 1e-9);
}

// ─── Per-box prompts and masks named in text (PixelGenie 3D doc §4, P2) ──────

/// Recraft's text layout is "supported by Recraft V3 and Recraft V3 Vector
/// models only", one word per element (endpoints.md, 2026-10-30); Stability's
/// Search and Replace takes `search_prompt` "maxLength": 10000 beside a
/// required prompt, for "5" credits (v2alpha openapi, pricing.md).
#[test]
fn recraft_v3_lays_out_single_words_and_stability_replaces_a_named_area() {
    use litegen::capabilities::{LayoutKind, ParamSpec};
    let reg = registry();
    for m in reg.for_provider("recraft") {
        let takes = matches!(m.params.get("layout"), Some(ParamSpec::Layout(_)));
        assert_eq!(takes, ["recraft/recraftv3", "recraft/recraftv3_vector"].contains(&m.id.as_str()), "{}", m.id);
        if let Some(ParamSpec::Layout(l)) = m.params.get("layout") {
            assert_eq!((l.kinds.as_slice(), l.single_word_text), ([LayoutKind::Text].as_slice(), true), "{}", m.id);
            assert!(!m.extra_allowlist.iter().any(|k| k == "text_layout"), "{}: litegen owns text_layout", m.id);
        }
    }
    let s = reg.get("stability/search-and-replace").expect("the row");
    match s.params.get("mask_prompt") {
        Some(ParamSpec::String(p)) => assert_eq!(p.max_length, Some(10_000)),
        other => panic!("{other:?}"),
    }
    assert!(s.prompt.required);
    assert!((s.pricing.base_cost_usd - 0.05).abs() < 1e-9);
}

// ─── Background swap (PixelGenie 3D doc §4, P2) ─────────────────────────────

/// Recraft's Replace Background: "$0.04 / $0.08 per image, raster / vector";
/// prompt "Up to 1,000 characters"; image_format webp | png
/// (tools/background.md, 2026-10-30).
#[test]
fn recraft_replace_background_is_a_background_swap_at_the_raster_price() {
    use litegen::capabilities::{ModelTask, ParamSpec};
    let reg = registry();
    let m = reg.get("recraft/replace-background").expect("the row");
    assert_eq!(m.task, ModelTask::BackgroundSwap);
    assert_eq!((m.prompt.required, m.prompt.max_length), (true, Some(1000)));
    match m.params.get("output_format") {
        Some(ParamSpec::String(f)) => assert_eq!(f.enum_values, ["png", "webp"]),
        other => panic!("{other:?}"),
    }
    assert!((m.pricing.base_cost_usd - 0.04).abs() < 1e-9);
}

// ─── Native pixel art (PixelGenie L16) ──────────────────────────────────────

/// PixelLab's pixflux: `image_size` "minimum": 16, "maximum": 400 a side;
/// `color_image` the "Forced color palette"
/// (<https://api.pixellab.ai/v2/openapi.json>, 2026-10-30). Recraft's
/// `controls.colors` is "An array of preferable colors", "at most 10"
/// (endpoints.md): a hint on every generation row.
#[test]
fn pixflux_forces_its_palette_at_native_size_and_recraft_takes_one_as_a_hint() {
    use litegen::capabilities::{ParamSpec, SizeSpec};
    let reg = registry();
    let m = reg.get("pixellab/pixflux").expect("the row");
    assert!(m.capabilities.native_pixel_art);
    match m.params.get("size") {
        Some(ParamSpec::Size(SizeSpec::Freeform(f))) => assert_eq!((f.min_width, f.max_width, f.min_height, f.max_height), (16, 400, 16, 400)),
        other => panic!("{other:?}"),
    }
    assert!(matches!(m.params.get("palette"), Some(ParamSpec::Palette(p)) if p.forced));
    for r in reg.for_provider("recraft").filter(|r| r.task == litegen::capabilities::ModelTask::Generate && !r.capabilities.background_removal) {
        assert!(matches!(r.params.get("palette"), Some(ParamSpec::Palette(p)) if !p.forced && p.max_colors == 10), "{}", r.id);
    }
}

// ─── fal audit, 2026-10-30: F13, F14, F15, F17 ─────────────────────────────

/// F14: `fal-ai/flux/schnell` takes 1-12 steps (default 4) and a guidance
/// scale of 1-20 (default 3.5); the row allowed 4 steps and no guidance.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux/schnell> (2026-10-30)
#[test]
fn fal_flux_schnell_takes_up_to_twelve_steps_and_a_guidance_scale() {
    let reg = registry();
    let s = reg.get("fal/flux-schnell").expect("fal/flux-schnell");
    match (s.params.get("steps"), s.params.get("guidance_scale")) {
        (Some(ParamSpec::Int(steps)), Some(ParamSpec::Float(g))) => {
            assert_eq!((steps.min, steps.max, steps.default), (Some(1), Some(12), Some(4)));
            assert_eq!((g.min, g.max, g.default), (Some(1.0), Some(20.0), Some(3.5)));
        }
        other => panic!("{other:?}"),
    }
}

/// F15: each row promises its endpoint's own bounds.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/flux/dev> (2026-10-30)
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/fast-sdxl> (2026-10-30)
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/stable-diffusion-v35-medium> (2026-10-30)
#[test]
fn fal_image_rows_take_their_endpoints_whole_ranges() {
    let reg = registry();
    for (id, steps, guidance) in [
        ("fal/flux-dev", (1, 50), (1.0, 20.0)),
        ("fal/sdxl", (1, 50), (0.0, 20.0)),
        ("fal/sd35-medium", (1, 50), (0.0, 20.0)),
    ] {
        let s = reg.get(id).expect(id);
        match (s.params.get("steps"), s.params.get("guidance_scale")) {
            (Some(ParamSpec::Int(st)), Some(ParamSpec::Float(g))) => {
                assert_eq!((st.min, st.max), (Some(steps.0), Some(steps.1)), "{id} steps");
                assert_eq!((g.min, g.max), (Some(guidance.0), Some(guidance.1)), "{id} guidance_scale");
            }
            other => panic!("{id}: {other:?}"),
        }
    }
}

/// F13: "Your request will cost $0.04 per image (or $0.08 if you are using a
/// vector style)." The row billed a vector image at $0.04.
/// @see <https://fal.ai/models/fal-ai/recraft/v3/text-to-image/llms.txt> (2026-10-30)
#[test]
fn fal_recraft_v3_bills_a_vector_style_twice() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    let m = reg.get("fal/recraft-v3").expect("fal/recraft-v3");
    for (style, usd) in [("vector_illustration", 0.08), ("realistic_image", 0.04), ("digital_illustration", 0.04)] {
        let req = serde_json::json!({ "model": m.id, "prompt": "x", "style": style });
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        assert!((unit_price_usd(m, &PriceFacts::new(m, &base, &req)) - usd).abs() < 1e-9, "{style}");
    }
}

/// F17: every fal row names the endpoint page its fields were read from.
#[test]
fn every_fal_row_names_its_endpoint_page() {
    for m in registry().all().filter(|m| m.provider == "fal") {
        let url = m.docs_url.as_deref().unwrap_or_else(|| panic!("{} names no docs_url", m.id));
        assert!(url.starts_with("https://fal.ai/models/") && url.ends_with("/api"), "{}: {url}", m.id);
    }
}

// ─── Google / OpenAI audit, 2026-10-30: C2, C3, S5 ─────────────────────────

/// C2: "Supported `image_size` values: `1024px` (1K). (Note: 2K and 4K are
/// unsupported)." The lite row let `imageSize` through.
/// @see <https://ai.google.dev/gemini-api/docs/models/gemini-3.1-flash-lite-image> (2026-10-30)
#[test]
fn gemini_flash_lite_image_takes_no_image_size() {
    let reg = registry();
    let lite = reg.get("google/gemini-3.1-flash-lite-image").expect("lite");
    assert!(!lite.extra_allowlist.iter().any(|k| k == "imageSize"), "{:?}", lite.extra_allowlist);
    assert!(!lite.params.contains_key("resolution"), "1K only: no tier to pick");
}

/// One output tier for every image vendor (2026-11-03): `resolution`, on
/// litegen's scale (`0.5K` … `4K`, about that many thousand pixels a side),
/// wherever a vendor has tiers, never a vendor-named extra. Google's
/// imageSize: "Supported values are `512`, `1K`, `2K`, `4K`" (3.1 Flash
/// Image "adds the smaller 512px (0.5K) resolution (not supported on Gemini
/// Nano Banana 2.1)"); Kling Image 2.1 / 3.0: "1k (default) | 2k"; FLUX 3:
/// "`768sq`, `1k` (default), `1.5k`, `2k`, or `4k`", FLUX 3 Image also
/// `match_input`.
/// @see <https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta> (ImageConfig, 2026-11-03)
/// @see <https://ai.google.dev/gemini-api/docs/generate-content/image-generation> (2026-11-03)
/// @see <https://kling.ai/document-api/api/image/2-1/image-generation.md> (2026-11-03)
/// @see <https://docs.bfl.ml/api-reference/utility/generate-an-image-with-flux-3> (2026-11-03)
#[test]
fn image_tiers_are_one_resolution_param_on_one_scale() {
    let reg = registry();
    // 3K: Runway's Seedream 5.0 Lite tier of 3072-a-side sizes (2026-11-10).
    let scale = ["0.5K", "0.75K", "1K", "1.5K", "2K", "3K", "4K", "match_input"];
    for m in reg.all().filter(|m| m.media_type == litegen::capabilities::MediaType::Image) {
        for k in ["imageSize", "image_size", "resolution"] {
            assert!(!m.extra_allowlist.iter().any(|a| a == k), "{}: the tier is `resolution`, not extra.{k}", m.id);
        }
        if let Some(ParamSpec::String(s)) = m.params.get("resolution") {
            assert!(s.enum_values.iter().all(|v| scale.contains(&v.as_str())), "{}: {:?}", m.id, s.enum_values);
            // The vendor's own default: 1K on every row but TokenHub's Hy-Image-3.5
            // ("2359296（1.5K，默认）") and TokenHub's Seedream ("默认值：2K").
            let want = match m.id.as_str() {
                "tokenhub/hy-image-v3.5-preview" => "1.5K",
                id if id.starts_with("tokenhub/seedream-") => "2K",
                // Runway's Seedream 5.0 Lite has no 1K tier: its `ratio`
                // enum starts at 2048:2048 (openapi.json, 2026-11-10).
                "runway/seedream5_lite" => "2K",
                _ => "1K",
            };
            assert_eq!(s.default.as_deref(), Some(want), "{}: the vendor's default tier", m.id);
            assert!(s.enum_values.iter().any(|v| Some(v.as_str()) == s.default.as_deref()), "{}: the default is a tier", m.id);
        }
    }
    for (id, want) in [
        ("google/gemini-3.1-flash-image", &["0.5K", "1K", "2K", "4K"][..]),
        ("google/gemini-nano-banana-2.1", &["1K", "2K", "4K"][..]),
        ("google/gemini-3-pro-image", &["1K", "2K", "4K"][..]),
        ("kling/kling-v2-1", &["1K", "2K"][..]),
        ("kling/kling-v3", &["1K", "2K"][..]),
        ("bfl/flux-3-image", &["0.75K", "1K", "1.5K", "2K", "4K", "match_input"][..]),
        ("bfl/flux-3-image-klein", &["0.75K", "1K", "1.5K", "2K", "4K"][..]),
        ("replicate/flux-3-image", &["0.75K", "1K", "1.5K", "2K", "4K"][..]),
        ("tokenhub/hy-image-v3.5-preview", &["1K", "1.5K", "2K"][..]),
        ("tokenhub/seedream-image-v5.0-pro", &["1K", "1.5K", "2K"][..]),
    ] {
        assert_eq!(enum_values(&reg, id, "resolution"), want, "{id}");
    }
}

/// C3: every Google and OpenAI row names the vendor page its fields were
/// read from. Google's docs moved from ai.google.dev to aistudio.google.com:
/// `https://ai.google.dev/gemini-api/docs/generate-content/image-generation`
/// answers 301 to `https://aistudio.google.com/docs/image-generation?generate-content=true`.
/// @see <https://aistudio.google.com/docs/image-generation?generate-content=true> (2026-11-09)
#[test]
fn every_google_and_openai_row_names_its_docs_url() {
    for m in registry().all().filter(|m| m.provider == "google" || m.provider == "openai") {
        let url = m.docs_url.as_deref().unwrap_or_else(|| panic!("{} names no docs_url", m.id));
        let home = if m.provider == "google" { "https://aistudio.google.com/docs/" } else { "https://developers.openai.com/" };
        assert!(url.starts_with(home), "{}: {url}", m.id);
    }
}

/// GPT image edits: "You can use one or more images as a reference to
/// generate a new image" — `image` is "an array of images" (maxItems 16) —
/// and "If you provide multiple input images, the mask will be applied to
/// the first image". So the edited image (`init`) plus up to 15 `reference`
/// images, and the mask; every row took one image.
/// @see <https://developers.openai.com/api/docs/guides/image-generation.md> (2026-11-03)
#[test]
fn openai_gpt_image_rows_take_up_to_sixteen_input_images() {
    let reg = registry();
    for id in ["openai/gpt-image-2", "openai/gpt-image-2.5-sunburst", "openai/gpt-image-2.5-flare"] {
        let ri = reg.get(id).unwrap_or_else(|| panic!("{id} missing")).ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} declares no ref_inputs"));
        assert_eq!(ri.roles.get("init").map(|r| r.max_count), Some(1), "{id}: one edited image");
        assert_eq!(ri.roles.get("reference").map(|r| (r.required, r.max_count)), Some((false, 15)), "{id}: up to 15 more");
        assert_eq!(ri.max_total, 17, "{id}: 16 images and a mask");
        match &ri.provider_format {
            litegen::capabilities::schema::RefProviderFormat::Multipart(m) => {
                assert_eq!(m.field_map.get("reference").map(String::as_str), Some("image"), "{id}: every image is an `image` part")
            }
            other => panic!("{id} refs go as {other:?}"),
        }
    }
}

/// S5: "The Images API `user` parameter is deprecated. Send the same
/// identifier as `safety_identifier` instead."
/// @see <https://developers.openai.com/api/docs/guides/image-generation> (2026-10-30)
#[test]
fn openai_rows_take_a_safety_identifier() {
    for m in registry().all().filter(|m| m.provider == "openai") {
        assert!(m.extra_allowlist.iter().any(|k| k == "safety_identifier"), "{}: {:?}", m.id, m.extra_allowlist);
    }
}

// ─── ByteDance / MiniMax / Vidu audit, 2026-10-30: W8, S7 ──────────────────

/// W8: Seedream answers inline (`b64_json`); `response_format: url` would have
/// litegen fetch the image server-side, so it is not a caller's knob.
#[test]
fn seedream_rows_do_not_allowlist_response_format() {
    for m in registry().all().filter(|m| m.provider == "bytedance" && m.media_type == litegen::capabilities::MediaType::Image) {
        assert!(!m.extra_allowlist.iter().any(|k| k == "response_format"), "{}: {:?}", m.id, m.extra_allowlist);
    }
}

/// S7: every ByteDance, MiniMax and Vidu row names the vendor page its fields
/// were read from.
#[test]
fn every_bytedance_minimax_and_vidu_row_names_its_docs_url() {
    for m in registry().all().filter(|m| ["bytedance", "minimax", "vidu"].contains(&m.provider.as_str())) {
        let url = m.docs_url.as_deref().unwrap_or_else(|| panic!("{} names no docs_url", m.id));
        assert!(
            ["https://docs.byteplus.com/", "https://platform.minimax.io/", "https://platform.vidu.com/"].iter().any(|p| url.starts_with(p)),
            "{}: {url}",
            m.id
        );
    }
}

// ─── Kling / PixVerse / Hunyuan / Luma audit, 2026-10-30 ───────────────────

/// P1: transition/generate's model enum is "c1 - v6 - v5.6 - v5", and the
/// capability map's "First and Last Frame" row has no v4.5 or v3.5.
/// @see <https://docs.platform.pixverse.ai/first-and-last-frame-video-43051213e0.md> (2026-10-30)
/// @see <https://docs.platform.pixverse.ai/model-capability-map-2394982m0.md> (2026-10-30)
#[test]
fn pixverse_v4_5_and_v3_5_take_no_last_frame() {
    let reg = registry();
    for id in ["pixverse/v4.5", "pixverse/v3.5"] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        let refs = m.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} takes no refs"));
        assert!(!refs.roles.contains_key("last_frame"), "{id}: {:?}", refs.roles.keys());
        assert_eq!(refs.max_total, 1, "{id}");
        assert!(!m.description.contains("transition"), "{id}: {}", m.description);
    }
}

/// P2: img/generate's prompt is "maxLength: 2048" (text/ and transition/
/// allow 5000), and one prompt limit serves every mode.
/// @see <https://docs.platform.pixverse.ai/image-to-video-and-video-templates-13016633e0.md> (2026-10-30)
#[test]
fn pixverse_prompts_stop_at_the_image_endpoint_s_2048() {
    for m in registry().all().filter(|m| m.provider == "pixverse") {
        assert_eq!(m.prompt.max_length, Some(2048), "{}", m.id);
    }
}

/// P3: text/, img/ and transition/ define no `negative_prompt`, `motion_mode`
/// or `style`, and `template_id` moves img/ onto per-template pricing
/// ("Use the selected template's `credit_consume` value") that a flat
/// `base_cost_usd` cannot bill.
/// @see <https://docs.platform.pixverse.ai/text-to-video-13016634e0.md> (2026-10-30)
/// @see <https://docs.platform.pixverse.ai/pixverse-api-pricing-2394995m0.md> (2026-10-30)
#[test]
fn pixverse_allowlists_name_only_fields_their_endpoints_bill_flat() {
    let reg = registry();
    for id in ["pixverse/v4.5", "pixverse/v5", "pixverse/v3.5", "pixverse/v6", "pixverse/c1"] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        for k in ["negative_prompt", "motion_mode", "style", "template_id"] {
            assert!(!m.extra_allowlist.iter().any(|x| x == k), "{id} allowlists {k}: {:?}", m.extra_allowlist);
        }
    }
}

/// H3: `Seed`: "不传：随机种子生成。 正数：固定种子生成。" (unset is random, a
/// positive seed is fixed), so 0 is not a seed.
/// @see <https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud/hunyuan/v20230901/models.go> (2026-10-30)
#[test]
fn hunyuan_image_seeds_are_positive() {
    let reg = registry();
    let m = reg.get("hunyuan/hunyuan-image").expect("hunyuan-image");
    match m.params.get("seed") {
        Some(ParamSpec::Seed(s)) => assert_eq!((s.min, s.max), (1, 2147483647)),
        other => panic!("seed is {other:?}"),
    }
}

/// H7: "生成一段5s，720p 的视频" and "生成一段5s，480p 的视频": both actions make
/// 5 s and take no duration, so another duration is refused instead of
/// billed and returned at 5 s.
/// @see <https://cloud.tencent.com/document/product/1616/118994> (2026-10-30)
#[test]
fn hunyuan_video_rows_are_five_seconds_only() {
    let reg = registry();
    for id in ["hunyuan/hunyuan-video", "hunyuan/hunyuan-video-i2v"] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Float(f)) => assert_eq!((f.min, f.max, f.default), (Some(5.0), Some(5.0), Some(5.0)), "{id}"),
            other => panic!("{id} duration_seconds is {other:?}"),
        }
    }
}

/// L7: `GenerationRequest.loop` is one boolean for every model; only a loop
/// with both keyframes is refused ("Loop is not supported for keyframes").
/// @see <https://docs.lumalabs.ai/reference/creategeneration> (2026-10-30)
/// @see <https://docs.lumalabs.ai/docs/errors.md> (2026-10-30)
#[test]
fn every_luma_video_row_takes_loop() {
    let reg = registry();
    for id in LUMA_DREAM_MACHINE_VIDEO {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(
            matches!(m.params.get("loop"), Some(ParamSpec::Bool(b)) if b.default == Some(false)),
            "{id} loop is {:?}",
            m.params.get("loop")
        );
    }
}

/// L8: "Prompt is too short, minimum length is 3 characters" and "Prompt is
/// too long, maximum length is 5000 characters".
/// @see <https://docs.lumalabs.ai/docs/errors.md> (2026-10-30)
#[test]
fn luma_video_prompts_are_three_to_five_thousand_characters() {
    let reg = registry();
    for id in LUMA_DREAM_MACHINE_VIDEO {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!((m.prompt.min_length, m.prompt.max_length), (Some(3), Some(5000)), "{id}");
    }
}

/// K17, P15, L13: every Kling, PixVerse, Hunyuan and Luma row names the
/// vendor page its fields were read from.
#[test]
fn every_kling_pixverse_hunyuan_and_luma_row_names_its_docs_url() {
    for m in registry().all().filter(|m| ["kling", "pixverse", "hunyuan", "luma"].contains(&m.provider.as_str())) {
        let url = m.docs_url.as_deref().unwrap_or_else(|| panic!("{} names no docs_url", m.id));
        assert!(
            ["https://kling.ai/document-api/", "https://docs.platform.pixverse.ai/", "https://cloud.tencent.com/document/", "https://docs.lumalabs.ai/"]
                .iter()
                .any(|p| url.starts_with(p)),
            "{}: {url}",
            m.id
        );
    }
}

/// P10: img/generate: "Duration in seconds. C1/V6: 1-15; V5.5/V5.6: 5, 8, or
/// 10; older models: 5 or 8.", and the capability map gives V5 "5 / 8
/// seconds", so 6 s and 7 s are refused here rather than at PixVerse.
/// @see <https://docs.platform.pixverse.ai/image-to-video-and-video-templates-13016633e0.md> (2026-10-30)
#[test]
fn pixverse_v5_and_older_take_five_or_eight_seconds() {
    let reg = registry();
    for id in ["pixverse/v3.5", "pixverse/v4.5", "pixverse/v5"] {
        match reg.get(id).unwrap_or_else(|| panic!("{id} missing")).params.get("duration_seconds") {
            Some(ParamSpec::Float(f)) => assert_eq!(f.values, vec![5.0, 8.0], "{id}"),
            other => panic!("{id} duration_seconds is {other:?}"),
        }
    }
}

/// "图生视频 | 通过上传图片和输入提示词，生成一段5s，480p 的视频 | 一次消耗2积分": a
/// 480p job is 2 credits (2.4 CNY, $0.358), not the 5 the default 720p costs.
/// @see <https://cloud.tencent.com/document/product/1616/118994> (2026-10-30)
#[test]
fn a_480p_hunyuan_image_to_video_job_is_billed_at_two_credits() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    let m = reg.get("hunyuan/hunyuan-video-i2v").expect("hunyuan-video-i2v");
    for (resolution, usd) in [("480p", 0.358), ("720p", 0.894), ("1080p", 0.894)] {
        let req = serde_json::json!({ "model": m.id, "prompt": "x", "resolution": resolution });
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        assert!((unit_price_usd(m, &PriceFacts::new(m, &base, &req)) - usd).abs() < 1e-9, "{resolution}");
    }
}

// ─── Runway / Leonardo / Replicate audit, 2026-10-30 ───────────────────────

/// #29: gen4_turbo on `/v1/image_to_video` is `required: ["promptImage",
/// "ratio", "model"]`; `promptText` is optional, "A non-empty string up to
/// 1000 characters".
/// @see <https://docs.dev.runwayml.com/openapi.json> (2026-10-30)
#[test]
fn runway_gen4_turbo_takes_an_optional_prompt() {
    let reg = registry();
    let schema = reg.get("runway/gen4-turbo").expect("runway/gen4-turbo");
    assert!(!schema.prompt.required, "promptText is not required for gen4_turbo");
    assert_eq!(schema.prompt.max_length, Some(1000));
}

/// #4, #33: `modelId` — "The model ID used for image generation" — would run
/// any Leonardo model at Diffusion XL's price, and `contrast` is "Used in
/// Phoenix and Flux models", not Diffusion XL.
/// @see <https://docs.leonardo.ai/v1.0/reference/creategeneration> (2026-10-30)
#[test]
fn leonardo_diffusion_xl_cannot_name_another_model_or_a_phoenix_knob() {
    let reg = registry();
    let schema = reg.get("leonardo/diffusion-xl").expect("leonardo/diffusion-xl");
    for key in ["modelId", "contrast"] {
        assert!(!schema.extra_allowlist.iter().any(|k| k == key), "allowlists {key}: {:?}", schema.extra_allowlist);
    }
}

/// #27: Motion 2.0 "Defaults to `RESOLUTION_480`", and litegen sends 720 when
/// no resolution is asked for; the row says so.
/// @see <https://docs.leonardo.ai/docs/motion-20.md> (2026-10-30)
#[test]
fn leonardo_motion2_says_it_is_sent_at_720p() {
    let reg = registry();
    let schema = reg.get("leonardo/motion2").expect("leonardo/motion2");
    match schema.params.get("resolution") {
        Some(ParamSpec::String(s)) => assert_eq!(s.default.as_deref(), Some("720p")),
        other => panic!("resolution is {other:?}"),
    }
}

/// #28: every Runway, Leonardo and Replicate row names the vendor page its
/// fields were read from.
#[test]
fn every_runway_leonardo_and_replicate_row_names_its_docs_url() {
    for m in registry().all().filter(|m| ["runway", "leonardo", "replicate"].contains(&m.provider.as_str())) {
        let url = m.docs_url.as_deref().unwrap_or_else(|| panic!("{} names no docs_url", m.id));
        assert!(
            ["https://docs.dev.runwayml.com/", "https://docs.leonardo.ai/", "https://replicate.com/"].iter().any(|p| url.starts_with(p)),
            "{}: {url}",
            m.id
        );
    }
}

// ─── BFL / Ideogram / Recraft / Stability audit, 2026-10-30 ────────────────

/// F14: "**Vector models** (every `_vector` model) accept the same 14 ratios
/// as `w:h` only; SVG output has no fixed pixel size."
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> (2026-10-30)
#[test]
fn recraft_vector_rows_take_no_pixel_size() {
    for m in registry().all().filter(|m| m.provider == "recraft" && m.id.contains("vector")) {
        assert!(!m.params.contains_key("size"), "{} declares a pixel size", m.id);
    }
}

/// F15: the Generate-image parameters are `prompt, n, model, size, style,
/// style_id, style_match, style_references, style_reference_urls,
/// negative_prompt, random_seed, response_format, image_format, text_layout,
/// controls`; there is no `substyle`.
/// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> (2026-10-30)
#[test]
fn recraft_rows_allowlist_no_substyle() {
    for m in registry().all().filter(|m| m.provider == "recraft") {
        assert!(!m.extra_allowlist.iter().any(|k| k == "substyle"), "{}: {:?}", m.id, m.extra_allowlist);
    }
}

/// F16: `icon` is a V2 style ("`icon` — V2 models only").
/// @see <https://www.recraft.ai/docs/api-reference/endpoints.md> (2026-10-30)
#[test]
fn recraft_v3_rows_offer_no_icon_style() {
    let reg = registry();
    for id in ["recraft/recraftv3", "recraft/recraftv3_vector"] {
        assert!(!enum_values(&reg, id, "style").iter().any(|v| v == "icon"), "{id}");
    }
}

/// F9: SDXL image-to-image takes the canonical `strength` (0 keeps the
/// image), sent inverted as `image_strength`.
/// @see <https://docs.stability.ai/openapi-v1.json> (2026-10-30)
#[test]
fn stability_sdxl_takes_an_image_to_image_strength() {
    let reg = registry();
    match reg.get("stability/sdxl").expect("stability/sdxl").params.get("strength") {
        Some(ParamSpec::Float(f)) => assert_eq!((f.min, f.max), (Some(0.0), Some(1.0))),
        other => panic!("strength is {other:?}"),
    }
}

/// F20: every BFL, Ideogram, Recraft and Stability row names the vendor page
/// its fields were read from (Stability's reference moved to
/// docs.stability.ai).
#[test]
fn every_bfl_ideogram_recraft_and_stability_row_names_its_docs_url() {
    for m in registry().all().filter(|m| ["bfl", "ideogram", "recraft", "stability"].contains(&m.provider.as_str())) {
        let url = m.docs_url.as_deref().unwrap_or_else(|| panic!("{} names no docs_url", m.id));
        assert!(
            ["https://docs.bfl.ml/", "https://developer.ideogram.ai/", "https://www.recraft.ai/docs/", "https://docs.stability.ai/"]
                .iter()
                .any(|p| url.starts_with(p)),
            "{}: {url}",
            m.id
        );
    }
}

/// 3D doc §4: "A mask-only vendor receives the box drawn as a mask" — so
/// every model that takes a mask takes `region` too, as its own box or drawn
/// as its mask; without it a region is refused (strict) or dropped (lax).
#[test]
fn every_model_that_takes_a_mask_takes_a_region() {
    let reg = registry();
    let mut lacking = Vec::new();
    for m in reg.served() {
        let takes_mask = m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("mask"));
        if takes_mask && !matches!(m.params.get("region"), Some(ParamSpec::Region(_))) {
            lacking.push(m.id.clone());
        }
    }
    assert!(lacking.is_empty(), "take a mask but no region: {lacking:?}");
}

// ─── tokenhub: Tencent TokenHub, 2026-11-04 ──────────────────────────────────

/// Hy-Image-3.5-preview on TokenHub (`POST /v1/wand/hunyuan-image/v35-generation`,
/// synchronous): `messages` ("服务端会取数组中最后一个 role=user 的消息作为本轮生图指令"),
/// `size` ("宽、高均为正整数，取值范围 [256, 8192] … 面积不超过 16777216"), `seed`
/// ("int64 类型，范围 [0, 2^63-1]；为 0 或不传时服务端随机分配"), `generate_max_pixels`
/// ("1048576（1K）、2359296（1.5K，默认）、4194304（2K）… 仅在未传 size 时生效"),
/// `footnote`, `use_search_tool`; images "图片总数量 ≤ 20 张". Price: 1K and 2K
/// 15,000 tokens (¥0.15), 4K 20,000 tokens (¥0.20) at 10 ¥/M tokens; 1 CNY =
/// 0.14987 USD (ECB, 2026-11-03).
/// @see <https://cloud.tencent.com/document/product/1823/135745> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_hy_image_3_5_preview_is_carried_with_its_documented_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let id = "tokenhub/hy-image-v3.5-preview";
    let m = reg.get(id).expect("tokenhub/hy-image-v3.5-preview missing");
    assert_eq!(m.provider, "tokenhub");
    assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/135745"));
    assert!(m.capabilities.text_to_image && m.capabilities.image_to_image);
    assert!((m.pricing.base_cost_usd - 0.0225).abs() < 1e-9, "¥0.15 at 1K/1.5K/2K");
    let four_k = m.pricing.variable_pricing.iter().find(|r| r.when.contains_key("megapixels"));
    assert_eq!(four_k.map(|r| r.when.get("megapixels").cloned()), Some(Some(serde_json::json!({ "gt": 4.194304 }))), "above the 2K tier is 4K");
    assert!(four_k.and_then(|r| r.usd).is_some_and(|p| (p - 0.03).abs() < 1e-9), "¥0.20 at 4K");
    match m.params.get("size") {
        Some(ParamSpec::Size(SizeSpec::Freeform(s))) => {
            assert_eq!((s.min_width, s.max_width, s.min_height, s.max_height), (256, 8192, 256, 8192));
            assert_eq!(s.max_pixels, Some(16_777_216));
        }
        other => panic!("{id} size: {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "resolution"), ["1K", "1.5K", "2K"], "generate_max_pixels' tiers; 4K only as a size");
    match m.params.get("resolution") {
        Some(ParamSpec::String(s)) => assert_eq!(s.default.as_deref(), Some("1.5K"), "\"2359296（1.5K，默认）\""),
        other => panic!("{id} resolution: {other:?}"),
    }
    assert!(!aspect_ratios(&reg, id).is_empty(), "a ratio goes as a size beside generate_max_pixels");
    match m.params.get("seed") {
        Some(ParamSpec::Seed(s)) => assert_eq!((s.min, s.max), (1, i64::MAX), "0 is the vendor's random seed"),
        other => panic!("{id} seed: {other:?}"),
    }
    for k in ["negative_prompt", "steps", "guidance_scale", "quality"] {
        assert!(!m.params.contains_key(k), "{id} declares no {k}");
    }
    let refs = m.ref_inputs.as_ref().expect("refs");
    assert_eq!((refs.max_total, refs.roles.get("init").map(|r| r.max_count)), (20, Some(20)));
    assert_eq!(m.extra_allowlist, ["footnote", "use_search_tool"]);
}

/// Hy-Video 1.5 on TokenHub (`POST /v1/wand/hunyuan-video/generation`,
/// `model` "hy-video-v1.5"): prompt "文生视频必选；图生视频可选", negative_prompt,
/// one image ("图片（单张）… jpg/jpeg/png"), duration "当前版本固定生成 5 秒",
/// resolution "当前版本固定输出 720p", aspect_ratio "仅文生视频有效。枚举：16:9 / 9:16 /
/// 1:1 / 4:3 / 3:4". Price "30,000 tokens/秒 | 0.3 元/秒": 5 s = ¥1.5.
/// @see <https://cloud.tencent.com/document/product/1823/137202> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_hy_video_1_5_is_carried_with_its_documented_contract() {
    let reg = registry();
    let id = "tokenhub/hy-video-v1.5";
    let m = reg.get(id).expect("tokenhub/hy-video-v1.5 missing");
    assert_eq!(m.provider, "tokenhub");
    assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/137202"));
    assert!(m.capabilities.text_to_video && m.capabilities.image_to_video);
    assert!((m.pricing.base_cost_usd - 0.2248).abs() < 1e-9, "¥1.5 for 5 s");
    match m.params.get("duration_seconds") {
        Some(ParamSpec::Float(d)) => assert_eq!((d.min, d.max, d.default), (Some(5.0), Some(5.0), Some(5.0))),
        other => panic!("{id} duration_seconds: {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "resolution"), ["720p"]);
    assert_eq!(aspect_ratios(&reg, id), ["16:9", "9:16", "1:1", "4:3", "3:4"]);
    assert!(m.params.contains_key("negative_prompt"));
    assert!(!m.params.contains_key("seed"), "the vendor does not promise its effect");
    let refs = m.ref_inputs.as_ref().expect("refs");
    assert_eq!((refs.max_total, refs.roles.get("first_frame").map(|r| (r.required, r.max_count))), (1, Some((false, 1))));
    assert!(!refs.roles.contains_key("last_frame"));
    assert_eq!(m.extra_allowlist, ["revise", "footnote"]);
}

/// WAND-Vega-Video-1.0-Lite on TokenHub (`POST /v1/wand/vega-videos/tasks`):
/// "三种生成形态（文生、首尾帧生、参考生）"; resolution "Lite 取值：768P、1080P、2K、4K"
/// (no documented default: litegen sends 768P); ratio "21:9、16:9、4:3、1:1、
/// 3:4、9:16、adaptive（默认）"; duration "4 ~ 15 的整数，默认 5"; "首帧（role=first_frame）
/// 最多 1 张，尾帧（role=last_frame）最多 1 张"; "建议参考图不超过 9 张". Price per output
/// second: 768P ¥0.375, 1080P ¥0.5, 2K ¥0.625, 4K ¥0.75; "输入图片前5张免费
/// 第6张起0.15元/张".
/// @see <https://cloud.tencent.com/document/product/1823/138195> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_wand_vega_video_lite_is_carried_with_its_documented_contract() {
    let reg = registry();
    let id = "tokenhub/wand-vega-video-1.0-lite";
    let m = reg.get(id).expect("tokenhub/wand-vega-video-1.0-lite missing");
    assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/138195"));
    assert!(m.capabilities.text_to_video && m.capabilities.image_to_video);
    assert!((m.pricing.base_cost_usd - 0.281).abs() < 1e-9, "5 s at 768P: ¥1.875");
    match m.params.get("duration_seconds") {
        Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(4), Some(15), Some(5))),
        other => panic!("{id} duration_seconds: {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "resolution"), ["768P", "1080P", "2K", "4K"]);
    assert_eq!(aspect_ratios(&reg, id), ["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"]);
    let per_second = |res: Option<&str>| {
        m.pricing.variable_pricing.iter().find(|r| r.when.get("resolution").and_then(|v| v.as_str()) == res && r.per_second_usd.is_some()).and_then(|r| r.per_second_usd)
    };
    for (res, usd) in [(None, 0.0562), (Some("1080P"), 0.0749), (Some("2K"), 0.0937), (Some("4K"), 0.1124)] {
        assert!(per_second(res).is_some_and(|p| (p - usd).abs() < 1e-9), "{res:?} is ${usd}/s");
    }
    let refs = m.ref_inputs.as_ref().expect("refs");
    assert_eq!(refs.roles.get("first_frame").map(|r| r.max_count), Some(1));
    assert_eq!(refs.roles.get("last_frame").map(|r| r.max_count), Some(1));
    assert_eq!(refs.roles.get("reference").map(|r| r.max_count), Some(9));
    for n in 6..=9 {
        let rule = m.pricing.variable_pricing.iter().find(|r| r.when.get("references") == Some(&serde_json::json!(n)));
        assert!(rule.and_then(|r| r.add_usd).is_some_and(|a| (a - 0.0225 * (n - 5) as f64).abs() < 1e-9), "{n} images: the 6th on is ¥0.15 each");
    }
}

// ─── bedrock: Stability on Bedrock, 2026-11-04 ───────────────────────────────

/// The three Stability text-to-image models Bedrock hosts (InvokeModel,
/// Stability's own body; "Support for all other Stability AI models are in
/// the process of being deprecated"): SD3.5 Large (prompt and negative prompt
/// "Maximum 10,000 characters", aspect_ratio "Enum: 16:9, 1:1, 21:9, 2:3, 3:2,
/// 4:5, 5:4, 9:16, 9:21. Default 1:1", mode text-to-image | image-to-image
/// with image + strength, seed 0–4294967294, output_format jpeg | png | webp);
/// Stable Image Core (text-to-image only, JPEG / PNG, seed 0–4294967295); Stable
/// Image Ultra (+ image and strength "Default: 0.35"). Prices "per generated
/// image": $0.08, $0.04, $0.14 (AWS's metered unit map, us-west-2). Each card:
/// "us-west-2 (Oregon)" In-Region only.
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-stability-diffusion.md> (2026-11-04)
/// @see <https://aws.amazon.com/bedrock/pricing/> (2026-11-04)
#[test]
fn bedrock_stability_rows_are_carried_with_their_documented_contract() {
    let reg = registry();
    for (id, usd, i2i, formats, seed_max) in [
        ("bedrock/stability.sd3-5-large-v1:0", 0.08, true, &["png", "jpeg", "webp"][..], 4_294_967_294i64),
        ("bedrock/stability.stable-image-core-v1:1", 0.04, false, &["png", "jpeg"][..], 4_294_967_295),
        ("bedrock/stability.stable-image-ultra-v1:1", 0.14, true, &["png", "jpeg"][..], 4_294_967_295),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(m.docs_url.as_deref().is_some_and(|u| u.starts_with("https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-")), "{id}");
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id} is ${usd}");
        assert!(m.capabilities.text_to_image);
        assert_eq!(m.capabilities.image_to_image, i2i, "{id}");
        assert_eq!(m.ref_inputs.as_ref().map(|r| r.max_total).unwrap_or(0), u32::from(i2i), "{id}: one init image exactly when it does image-to-image");
        assert_eq!(m.params.contains_key("strength"), i2i, "{id}");
        assert_eq!(enum_values(&reg, id, "output_format"), formats, "{id}");
        assert_eq!(aspect_ratios(&reg, id), ["1:1", "16:9", "21:9", "2:3", "3:2", "4:5", "5:4", "9:16", "9:21"], "{id}");
        match m.params.get("seed") {
            Some(ParamSpec::Seed(s)) => assert_eq!((s.min, s.max), (0, seed_max), "{id}"),
            other => panic!("{id} seed: {other:?}"),
        }
        assert_eq!(m.prompt.max_length, Some(10_000), "{id}");
        for k in ["steps", "guidance_scale", "size", "style"] {
            assert!(!m.params.contains_key(k), "{id} declares no {k}");
        }
    }
}

/// Luma Ray 2 on Bedrock (`StartAsyncInvoke`, the MP4 written to the
/// caller's S3 bucket), carried beside luma/ray-2 under the hub-route policy
/// (2026-11-04):
/// - prompt "1–5,000 characters";
/// - aspect_ratio "1:1", "16:9", "9:16", "4:3", "3:4", "21:9", "9:21";
/// - duration "5s", "9s"; resolution "540p", "720p";
/// - `loop`; keyframes frame0 and frame1, base64 JPEG or PNG.
///
/// Price per second of video: 720p $1.50, 540p $0.75 (AWS's metered unit
/// map; US West (Oregon) only, the card's one In-Region endpoint).
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-luma.md> (2026-11-10)
/// @see <https://docs.aws.amazon.com/bedrock/latest/userguide/model-card-luma-ai-ray2.md> (2026-11-10)
/// @see <https://aws.amazon.com/bedrock/pricing/> (2026-11-10)
#[test]
fn bedrock_luma_ray_2_is_carried_with_bedrocks_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    let id = "bedrock/luma.ray-v2:0";
    let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
    assert_eq!(m.media_type, litegen::capabilities::MediaType::Video);
    assert_eq!(m.docs_url.as_deref(), Some("https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-luma.html"));
    assert!(m.capabilities.text_to_video && m.capabilities.image_to_video);
    assert_eq!(m.prompt.max_length, Some(5000));
    assert_eq!(aspect_ratios(&reg, id), ["1:1", "16:9", "9:16", "4:3", "3:4", "21:9", "9:21"]);
    assert_eq!(enum_values(&reg, id, "resolution"), ["540p", "720p"]);
    match m.params.get("duration_seconds") {
        Some(ParamSpec::Float(d)) => assert_eq!((d.min, d.max, d.default, d.values.clone()), (Some(5.0), Some(9.0), Some(5.0), vec![5.0, 9.0])),
        other => panic!("duration_seconds: {other:?}"),
    }
    assert!(matches!(m.params.get("loop"), Some(ParamSpec::Bool(_))));
    let refs = m.ref_inputs.as_ref().expect("keyframes");
    assert_eq!((refs.roles["first_frame"].max_count, refs.roles["last_frame"].max_count), (1, 1));
    assert!(refs.roles.values().all(|r| !r.required), "text-to-video needs no frame");
    let price = |params: serde_json::Value| {
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        req.as_object_mut().unwrap().extend(params.as_object().unwrap().clone());
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    for (params, usd) in [
        (serde_json::json!({}), 7.5),
        (serde_json::json!({ "duration_seconds": 9 }), 13.5),
        (serde_json::json!({ "resolution": "540p" }), 3.75),
        (serde_json::json!({ "resolution": "540p", "duration_seconds": 9 }), 6.75),
    ] {
        let got = price(params.clone());
        assert!((got - usd).abs() < 1e-9, "{params}: {got} (want {usd})");
    }
}

/// Kling video resold on TokenHub (`POST /v1/wand/kling/text-to-video`,
/// `/image-to-video`, `model` in the body): "v4：3~30 整数；V3 / V3 Turbo：3~15 整数；
/// V2.6 / V2.5 Turbo：5 / 10"; resolution "V3：720p / 1080p / 4k；其余：720p / 1080p。
/// 默认值：v4 为 1080p，其余为 720p"; aspect_ratio "16:9 / 9:16 / 1:1（v4 另支持 21:9）";
/// audio "仅 V3 / V2.6"; types "V3 Turbo：prompt / first_frame" (no last frame);
/// prompt "V3 Turbo / V2.6 / V2.5 Turbo：≤ 2500 字符", v4 "纯非英文 ≤ 11000 字符". Price
/// per second (10 ¥/M tokens): V2.5 Turbo 720P ¥0.3 / 1080P ¥0.5; V2.6 无声 ¥0.3 /
/// ¥0.5, 有声 1080P ¥1.0; V3 无声 ¥0.6 / ¥0.8 / 4K ¥3, 有声 ¥0.9 / ¥1.2 / ¥3; V3
/// Turbo 有声 ¥0.8 / ¥1.0; v4 ¥1 / ¥1.5.
/// @see <https://cloud.tencent.com/document/product/1823/135742> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_kling_video_rows_are_carried_with_tokenhubs_contract() {
    let reg = registry();
    let int = |id: &str| match reg.get(id).and_then(|m| m.params.get("duration_seconds")) {
        Some(ParamSpec::Int(d)) => (d.min, d.max, d.default, d.values.clone()),
        other => panic!("{id} duration_seconds: {other:?}"),
    };
    // (id, durations, values, resolutions, default resolution, ratios, audio, last frame, prompt limit, base, per second at the default)
    for (id, (min, max), values, res, res_default, ratios, audio, last, prompt_max, base) in [
        ("tokenhub/kling-video-v2.5-turbo", (5, 10), vec![5, 10], &["720p", "1080p"][..], "720p", &["16:9", "9:16", "1:1"][..], false, true, 2500, 0.225),
        ("tokenhub/kling-video-v2.6", (5, 10), vec![5, 10], &["720p", "1080p"][..], "720p", &["16:9", "9:16", "1:1"][..], true, true, 2500, 0.225),
        ("tokenhub/kling-video-v3", (3, 15), vec![], &["720p", "1080p", "4k"][..], "720p", &["16:9", "9:16", "1:1"][..], true, true, 2500, 0.4495),
        ("tokenhub/kling-video-v3-turbo", (3, 15), vec![], &["720p", "1080p"][..], "720p", &["16:9", "9:16", "1:1"][..], false, false, 2500, 0.5995),
        ("tokenhub/kling-video-v4", (3, 30), vec![], &["720p", "1080p"][..], "1080p", &["21:9", "16:9", "9:16", "1:1"][..], false, true, 11000, 1.124),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/135742"), "{id}");
        assert!(m.capabilities.text_to_video && m.capabilities.image_to_video, "{id}");
        assert_eq!(int(id), (Some(min), Some(max), Some(5), values), "{id}");
        assert_eq!(enum_values(&reg, id, "resolution"), res, "{id}");
        match m.params.get("resolution") {
            Some(ParamSpec::String(s)) => assert_eq!(s.default.as_deref(), Some(res_default), "{id}"),
            other => panic!("{id} resolution: {other:?}"),
        }
        assert_eq!(aspect_ratios(&reg, id), ratios, "{id}");
        assert_eq!(m.params.contains_key("audio"), audio, "{id}");
        let refs = m.ref_inputs.as_ref().expect("refs");
        assert_eq!(refs.roles.contains_key("last_frame"), last, "{id}");
        assert_eq!(refs.roles.get("first_frame").map(|r| r.max_count), Some(1), "{id}");
        assert_eq!(m.prompt.max_length, Some(prompt_max), "{id}");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: ${base} for its default request");
        assert!(!m.params.contains_key("negative_prompt"), "{id}: the prompt carries negatives");
    }
}

/// Seedream resold on TokenHub (`POST /v1/wand/si-image/generation`,
/// synchronous): prompt "≤ 600 字符"; images "seedream-image-v5.0-pro、
/// seedream-image-v5.0-flash 支持最多10 张"; size either a tier "可选值：1K、1.5K、2K"
/// ("默认值：2K") or "宽 x 高 … 总像素取值范围：[1280x720（921600）, 2048x2048x1.1025
/// （4624220）] 宽高比取值范围：[1/16, 16]", "不可混用"; output_format png | jpeg;
/// background transparent | opaque. Price: Pro "输出图≤261万像素 … 0.3 元/张", "&gt;261万像素
/// … 0.6 元/张", "输入图片首张免费，第二张起0.02 元/张"; Flash "0.12 元/张".
/// @see <https://cloud.tencent.com/document/product/1823/136609> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_seedream_rows_are_carried_with_tokenhubs_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    for (id, base) in [("tokenhub/seedream-image-v5.0-pro", 0.0899), ("tokenhub/seedream-image-v5.0-flash", 0.018)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/136609"), "{id}");
        assert!(m.capabilities.text_to_image && m.capabilities.image_to_image);
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: ${base} at its 2K default");
        assert_eq!(m.prompt.max_length, Some(600), "{id}");
        assert_eq!(enum_values(&reg, id, "resolution"), ["1K", "1.5K", "2K"], "{id}");
        match m.params.get("resolution") {
            Some(ParamSpec::String(s)) => assert_eq!(s.default.as_deref(), Some("2K"), "{id}: \"默认值：2K\""),
            other => panic!("{id} resolution: {other:?}"),
        }
        match m.params.get("size") {
            Some(ParamSpec::Size(SizeSpec::Freeform(s))) => {
                assert_eq!((s.min_pixels, s.max_pixels, s.max_aspect), (Some(921_600), Some(4_624_220), Some(16.0)), "{id}");
            }
            other => panic!("{id} size: {other:?}"),
        }
        assert_eq!(enum_values(&reg, id, "output_format"), ["png", "jpeg"], "{id}");
        assert_eq!(enum_values(&reg, id, "background"), ["transparent", "opaque"], "{id}");
        let refs = m.ref_inputs.as_ref().expect("refs");
        assert_eq!((refs.max_total, refs.roles.get("init").map(|r| r.max_count)), (10, Some(10)), "{id}");
    }
    let pro = reg.get("tokenhub/seedream-image-v5.0-pro").unwrap();
    let rule = |k: &str, v: serde_json::Value| pro.pricing.variable_pricing.iter().find(|r| r.when.get(k) == Some(&v)).cloned();
    assert!(rule("resolution", serde_json::json!("1.5K")).and_then(|r| r.usd).is_some_and(|u| (u - 0.045).abs() < 1e-9), "1.5K is ≤ 2.61 MP");
    assert!(rule("references", serde_json::json!(10)).and_then(|r| r.add_usd).is_some_and(|a| (a - 0.027).abs() < 1e-9), "nine billed input images");
}

/// MiniMax resold on TokenHub: v2.3 / v2.3-fast in the flat shape (`POST
/// /v1/wand/minimax-video/generation`: "768P：6 或 10；1080P：仅 6", resolution "768P（默认）
/// / 1080P", prompt "≤ 2000 字符"; v2.3-fast "仅支持图生"), H3 / H3-Max in the
/// `content` shape (`/v1/wand/minimax-video-v2/generation`: "h3：4 ~ 15 的整数。
/// h3-max：5 ~ 15", "h3：768P / 2K … h3-max：仅 480P / 768P", ratio "21:9 / 16:9 / 4:3 /
/// 1:1 / 3:4 / 9:16", H3 "参考图 ≤ 9 张"). Price: v2.3 6 s 768P ¥0.333333/s, 1080P
/// ¥0.583333/s, 10 s 768P ¥0.4/s; v2.3-fast ¥0.225 / ¥0.385 / ¥0.225; H3 768P ¥0.5/s,
/// 2K ¥0.8/s, "输入图片前5张免费第六张起0.2元/张"; H3-Max 480P ¥0.33/s, 768P ¥0.5/s.
/// @see <https://cloud.tencent.com/document/product/1823/135737> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_minimax_rows_are_carried_with_tokenhubs_contract() {
    let reg = registry();
    let int = |id: &str| match reg.get(id).and_then(|m| m.params.get("duration_seconds")) {
        Some(ParamSpec::Int(d)) => (d.min, d.max, d.default, d.values.clone()),
        other => panic!("{id} duration_seconds: {other:?}"),
    };
    for (id, durations, res, base, t2v, refs) in [
        ("tokenhub/minimax-video-v2.3", (Some(6), Some(10), Some(6), vec![6, 10]), &["768P", "1080P"][..], 0.2997, true, 0),
        ("tokenhub/minimax-video-v2.3-fast", (Some(6), Some(10), Some(6), vec![6, 10]), &["768P", "1080P"][..], 0.2023, false, 0),
        ("tokenhub/minimax-video-h3", (Some(4), Some(15), Some(5), vec![]), &["768P", "2K"][..], 0.3745, true, 9),
        ("tokenhub/minimax-video-h3-max", (Some(5), Some(15), Some(5), vec![]), &["480P", "768P"][..], 0.3745, true, 0),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/135737"), "{id}");
        assert_eq!(m.capabilities.text_to_video, t2v, "{id}");
        assert!(m.capabilities.image_to_video, "{id}");
        assert_eq!(int(id), durations, "{id}");
        assert_eq!(enum_values(&reg, id, "resolution"), res, "{id}");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: ${base}");
        let r = m.ref_inputs.as_ref().expect("refs");
        assert_eq!(r.roles.get("reference").map(|x| x.max_count).unwrap_or(0), refs, "{id}");
        assert_eq!(r.roles.get("first_frame").map(|x| x.required), Some(!t2v), "{id}: image-to-video only needs its first frame");
    }
    let h3 = reg.get("tokenhub/minimax-video-h3").unwrap();
    assert_eq!(aspect_ratios(&reg, "tokenhub/minimax-video-h3"), ["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"]);
    for n in 6..=9 {
        let rule = h3.pricing.variable_pricing.iter().find(|r| r.when.get("references") == Some(&serde_json::json!(n)));
        assert!(rule.and_then(|r| r.add_usd).is_some_and(|a| (a - 0.03 * (n - 5) as f64).abs() < 1e-9), "{n} images");
    }
}

/// PixVerse resold on TokenHub (`/v1/wand/pixverse/{text,image,start-end,
/// reference}-to-video`): duration "v5.6：5/8/10（1080p 不支持 10）；v6.0/c1：1~15。默认值：5",
/// quality "360p / 540p / 720p / 1080p", aspect_ratio "v5.6：16:9 / 4:3 / 1:1 / 3:4 /
/// 9:16；v6.0/c1 额外支持 2:3 / 3:2 / 21:9", seed "0 ~ 2147483647", image_references "最多支持 7
/// 项". Price per second at 540p silent: v6 ¥0.20531, v5.6 ¥0.20531, c1 ¥0.23464.
/// @see <https://cloud.tencent.com/document/product/1823/135325> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_pixverse_rows_are_carried_with_tokenhubs_contract() {
    let reg = registry();
    let wide = ["16:9", "4:3", "1:1", "3:4", "9:16", "2:3", "3:2", "21:9"];
    for (id, durations, ratios, base) in [
        ("tokenhub/pixverse-video-v6.0", (Some(1), Some(15), Some(5), vec![]), &wide[..], 0.154),
        ("tokenhub/pixverse-video-c1", (Some(1), Some(15), Some(5), vec![]), &wide[..], 0.176),
        ("tokenhub/pixverse-video-v5.6", (Some(5), Some(10), Some(5), vec![5, 8, 10]), &wide[..5], 0.154),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/135325"), "{id}");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default, d.values.clone()), durations, "{id}"),
            other => panic!("{id}: {other:?}"),
        }
        assert_eq!(enum_values(&reg, id, "resolution"), ["360p", "540p", "720p", "1080p"], "{id}");
        assert_eq!(aspect_ratios(&reg, id), ratios, "{id}");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: ${base} for 5 s at 540p silent");
        assert!(m.params.contains_key("audio"), "{id}");
        let refs = m.ref_inputs.as_ref().expect("refs");
        assert_eq!(refs.roles.get("reference").map(|r| r.max_count), Some(7), "{id}");
        assert!(refs.roles.contains_key("first_frame") && refs.roles.contains_key("last_frame"), "{id}");
    }
}

/// Kling and Vidu images resold on TokenHub (asynchronous: submit, then poll).
/// Kling (`/v1/wand/kling-image/generation`): v3 "image … 参考图片（单图）", o1 and
/// v3-omni "image_list … 最多 10 张"; resolution "1k … 2k" ("4K 仅 kling-image-v3-omni
/// 支持"); aspect_ratio "16:9、9:16、1:1、4:3、3:4、3:2、2:3、21:9"; prompt "≤ 2500 字符";
/// "0.2 元/张" (v3-omni 4K "0.4 元/张"). Vidu (`/v1/wand/vidu-image/generation`):
/// images "0张-7张", resolution "1080p / 2K / 4K" (litegen's 1K is its 1080p),
/// prompt "≤ 2000 字符"; 文生图 ¥0.1875 / ¥0.25 / ¥0.3125, 参考生图（1~3张）¥0.25 /
/// ¥0.375 / ¥0.625, （4~7张）¥0.3125 / ¥0.5 / ¥0.9375.
/// @see <https://cloud.tencent.com/document/product/1823/138803> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/135746> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_kling_and_vidu_image_rows_are_carried_with_tokenhubs_contract() {
    let reg = registry();
    for (id, tiers, refs, base) in [
        ("tokenhub/kling-image-v3", &["1K", "2K"][..], 1, 0.03),
        ("tokenhub/kling-image-o1", &["1K", "2K"][..], 10, 0.03),
        ("tokenhub/kling-image-v3-omni", &["1K", "2K", "4K"][..], 10, 0.03),
        ("tokenhub/vidu-image-q2", &["1K", "2K", "4K"][..], 7, 0.0281),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(m.docs_url.as_deref().is_some_and(|u| u.starts_with("https://cloud.tencent.com/document/product/1823/")), "{id}");
        assert!(m.capabilities.text_to_image && m.capabilities.image_to_image, "{id}");
        assert_eq!(enum_values(&reg, id, "resolution"), tiers, "{id}");
        assert_eq!(m.ref_inputs.as_ref().map(|r| r.max_total), Some(refs), "{id}");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: ${base}");
    }
    assert_eq!(aspect_ratios(&reg, "tokenhub/kling-image-v3"), ["16:9", "9:16", "1:1", "4:3", "3:4", "3:2", "2:3", "21:9"]);
    assert!(reg.get("tokenhub/kling-image-v3").unwrap().params.contains_key("negative_prompt"), "v3 text-to-image takes one");
    assert!(!reg.get("tokenhub/kling-image-o1").unwrap().params.contains_key("negative_prompt"), "omni: the prompt carries negatives");
    let vidu = reg.get("tokenhub/vidu-image-q2").unwrap();
    let price = |when: serde_json::Value| vidu.pricing.variable_pricing.iter().find(|r| serde_json::to_value(&r.when).ok() == Some(when.clone())).and_then(|r| r.usd);
    assert_eq!(price(serde_json::json!({ "references": { "gte": 4 }, "resolution": "4K" })), Some(0.1405), "4~7 references at 4K: ¥0.9375");
}

/// Vidu Q3 / Q4 video resold on TokenHub (`/v1/wand/vidu/{text,image,start-end,
/// reference}-to-video`): text-to-video "取值：vidu-video-q3-turbo、vidu-video-q3-pro、
/// vidu-video-q2"; image-to-video "vidu-video-q4-preview、vidu-video-q3-turbo、
/// vidu-video-q3-pro、vidu-video-q2-pro、vidu-video-q2-turbo"; reference
/// "vidu-video-q4-preview、vidu-video-q3-turbo、vidu-video-q3、vidu-video-q2-pro、
/// vidu-video-q2"; start-end "vidu-video-q3-turbo、vidu-video-q3-pro、vidu-video-q2-pro、
/// vidu-video-q2-turbo". Durations (速查表): q4-preview 3 ~ 16; q3 series 1 ~ 16
/// (references 3 ~ 16: one range serves every endpoint a row reaches). Resolution
/// 540p / 720p / 1080p (q4-preview + 2K / 4K).
/// Price per second: q4-preview ¥0.28125 / ¥0.59375 / ¥0.75 / ¥1.1875 / ¥2.4375;
/// q3-pro ¥0.28125 / ¥0.625 / ¥0.75; q3-turbo ¥0.21875 / ¥0.375 / ¥0.40625 (with
/// references ¥0.125 / ¥0.3125 / ¥0.40625); q3 ¥0.21875 / ¥0.375 / ¥0.46875.
/// @see <https://cloud.tencent.com/document/product/1823/135756> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_vidu_q3_and_q4_rows_are_carried_with_tokenhubs_contract() {
    let reg = registry();
    let caps = |id: &str| reg.get(id).map(|m| (m.capabilities.text_to_video, m.capabilities.image_to_video));
    for (id, t2v, durations, res, base) in [
        ("tokenhub/vidu-video-q4-preview", false, (3, 16), &["540p", "720p", "1080p", "2K", "4K"][..], 0.445),
        ("tokenhub/vidu-video-q3-pro", true, (1, 16), &["540p", "720p", "1080p"][..], 0.4685),
        ("tokenhub/vidu-video-q3-turbo", true, (3, 16), &["540p", "720p", "1080p"][..], 0.281),
        ("tokenhub/vidu-video-q3", false, (3, 16), &["540p", "720p", "1080p"][..], 0.281),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/135756"), "{id}");
        assert_eq!(caps(id), Some((t2v, true)), "{id}");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(durations.0), Some(durations.1), Some(5)), "{id}"),
            other => panic!("{id}: {other:?}"),
        }
        assert_eq!(enum_values(&reg, id, "resolution"), res, "{id}");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: ${base} for 5 s at its default 720p");
    }
    let refs = |id: &str| reg.get(id).and_then(|m| m.ref_inputs.as_ref()).map(|r| (r.roles.get("reference").map(|x| x.max_count), r.roles.contains_key("last_frame"), r.roles.contains_key("first_frame")));
    assert_eq!(refs("tokenhub/vidu-video-q4-preview"), Some((Some(15), false, true)), "\"参考图最多 15 张\"; no start-end");
    assert_eq!(refs("tokenhub/vidu-video-q3-pro"), Some((None, true, true)), "no reference endpoint");
    assert_eq!(refs("tokenhub/vidu-video-q3-turbo"), Some((Some(3), true, true)));
    assert_eq!(refs("tokenhub/vidu-video-q3"), Some((Some(3), false, false)), "reference-to-video only");
}

/// Vidu Q2 resold on TokenHub: q2 text and references (no audio: text-to-video
/// "仅 q3 系列支持", references "非主体调用时仅 q3 支持"), q2-pro image / start-end /
/// references (audio on image-to-video), q2-turbo image / start-end; "1 ~ 10"
/// (start-end "1 ~ 8", refused above 8 s).
/// @see <https://cloud.tencent.com/document/product/1823/135756> (2026-11-04)
#[test]
fn tokenhub_vidu_q2_rows_are_carried_with_tokenhubs_contract() {
    let reg = registry();
    for (id, t2v, audio, base) in [
        ("tokenhub/vidu-video-q2", true, false, 0.1638),
        ("tokenhub/vidu-video-q2-pro", false, true, 0.2574),
        ("tokenhub/vidu-video-q2-turbo", false, true, 0.2246),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.capabilities.text_to_video, t2v, "{id}");
        assert_eq!(m.params.contains_key("audio"), audio, "{id}");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: ${base}");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max), (Some(1), Some(10)), "{id}"),
            other => panic!("{id}: {other:?}"),
        }
    }
    assert_eq!(reg.get("tokenhub/vidu-video-q2-turbo").and_then(|m| m.ref_inputs.as_ref()).and_then(|r| r.roles.get("first_frame")).map(|r| r.required), Some(true));
}

/// YouTu image-to-video on TokenHub (`POST /v1/api/video/submit` | `/query`,
/// "model": "yt-video-2.0"): "请求的具体参数与原接口一致（YT-Video-2.0为 图生视频接口）… 参数统一
/// 以小写下划线形式呈现" — SubmitImageToVideoGeneralJob's Image, Prompt (≤ 200),
/// Resolution 480p | 720p | 1080p, Fps 16 | 24 | 30, a 5 s clip. "按次计算消耗。480p：
/// 2 积分/次 720p/1080p：5 积分/次" at 1.0 ¥ per credit.
/// @see <https://cloud.tencent.com/document/product/1823/135716> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_yt_video_2_0_is_carried_with_tokenhubs_contract() {
    let reg = registry();
    let id = "tokenhub/yt-video-2.0";
    let m = reg.get(id).expect("tokenhub/yt-video-2.0 missing");
    assert_eq!(m.docs_url.as_deref(), Some("https://cloud.tencent.com/document/product/1823/135716"));
    assert!(!m.capabilities.text_to_video && m.capabilities.image_to_video);
    assert!((m.pricing.base_cost_usd - 0.7494).abs() < 1e-9, "720p: 5 credits = ¥5");
    assert!(m.pricing.variable_pricing.iter().any(|r| r.when.get("resolution") == Some(&serde_json::json!("480p")) && r.usd.is_some_and(|u| (u - 0.2997).abs() < 1e-9)));
    assert_eq!(enum_values(&reg, id, "resolution"), ["480p", "720p", "1080p"]);
    assert_eq!(m.prompt.max_length, Some(200));
    assert_eq!(m.ref_inputs.as_ref().and_then(|r| r.roles.get("first_frame")).map(|r| r.required), Some(true));
}

/// Kling Omni resold on TokenHub (`POST /v1/wand/kling/omni-video`): "model … 取值：
/// kling-video-v4、kling-video-v3-omni、kling-video-o1"; resolution "V3 Omni 另支持 4k" (O1
/// 720p / 1080p); aspect_ratio "V3 Omni / O1：16:9 / 9:16 / 1:1，默认 16:9"; duration "V3
/// Omni：3~15；O1：3~10（O1 仅用首帧且无其他参考时仅支持 5 / 10）"; audio "V3 Omni：native /
/// original / off … O1：original / off" (original keeps a reference video's sound,
/// which litegen does not send). Carried with text and first/last frames, at the
/// "无参考视频" prices: V3 Omni silent ¥0.6 / ¥0.8 / ¥3, with sound ¥0.8 / ¥1 / ¥3; O1
/// ¥0.6 / ¥0.8 per second.
/// @see <https://cloud.tencent.com/document/product/1823/135742> (2026-11-04)
/// @see <https://cloud.tencent.com/document/product/1823/130055> (2026-11-04)
#[test]
fn tokenhub_kling_omni_rows_are_carried_with_text_and_frames() {
    let reg = registry();
    for (id, max, res, audio) in [
        ("tokenhub/kling-video-v3-omni", 15, &["720p", "1080p", "4k"][..], true),
        ("tokenhub/kling-video-o1", 10, &["720p", "1080p"][..], false),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(m.capabilities.text_to_video && m.capabilities.image_to_video, "{id}");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(3), Some(max), Some(5)), "{id}"),
            other => panic!("{id}: {other:?}"),
        }
        assert_eq!(enum_values(&reg, id, "resolution"), res, "{id}");
        assert_eq!(m.params.contains_key("audio"), audio, "{id}");
        assert!((m.pricing.base_cost_usd - 0.4495).abs() < 1e-9, "{id}: 5 s at 720p silent, ¥0.6/s");
        let refs = m.ref_inputs.as_ref().expect("refs");
        assert!(!refs.roles.contains_key("reference"), "{id}: refer_image's count is not documented for this model");
    }
}

/// P-Image on Ideogram v2 (`/v2/image/generate/p-image-ideogram`, JSON):
/// quality "very_low" … "very_high" ("Defaults to `medium`"), resolution
/// "1k" | "2k" ("Defaults to `1k`"), 15 aspect ratios, seed; text to image
/// only. Ideogram's price page (rendered 2026-11-04): "P-Image Ideogram Very
/// Low 1K … US $0.003" through "Very High 2K … US $0.066".
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-04)
/// @see <https://ideogram.ai/pricing/?pricing_tab=api> (rendered 2026-11-04)
#[test]
fn ideogram_p_image_is_carried_with_ideograms_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    let id = "ideogram/p-image-ideogram";
    let m = reg.get(id).expect("ideogram/p-image-ideogram missing");
    assert!(m.capabilities.text_to_image && !m.capabilities.image_to_image);
    assert_eq!(m.prompt.max_length, Some(10000));
    assert_eq!(enum_values(&reg, id, "quality"), ["very_low", "low", "medium", "high", "very_high"]);
    assert_eq!(enum_values(&reg, id, "resolution"), ["1K", "2K"]);
    assert_eq!(aspect_ratios(&reg, id).len(), 15);
    assert!(m.ref_inputs.is_none(), "text to image only");
    let price = |quality: Option<&str>, resolution: Option<&str>| {
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        if let Some(q) = quality {
            req["quality"] = q.into();
        }
        if let Some(r) = resolution {
            req["resolution"] = r.into();
        }
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    assert!((price(None, None) - 0.01).abs() < 1e-9, "medium, 1K by default");
    for (q, one, two) in [("very_low", 0.003, 0.006), ("low", 0.0075, 0.015), ("medium", 0.01, 0.02), ("high", 0.015, 0.03), ("very_high", 0.033, 0.066)] {
        assert!((price(Some(q), Some("1K")) - one).abs() < 1e-9, "{q} 1K");
        assert!((price(Some(q), Some("2K")) - two).abs() < 1e-9, "{q} 2K");
    }
}

/// The video models Ideogram resells on v2 (`POST /v2/video/generate/
/// {model}-{text,image,reference}-to-video`, polled at `GET /v2/generations/
/// {generation_id}`), carried as Ideogram routes (the hub-route policy,
/// 2026-11-04):
/// - Kling 3.0 Standard: prompt ≤ 2,000, duration 3–15, aspect 16x9 | 9x16 |
///   1x1, generate_audio, negative_prompt; "1080p | silent $0.084, audio $0.126"
///   a second.
/// - MiniMax H3 (Hailuo 3): duration 5–15, resolution 480P | 768P | 2K | 4K
///   (default 2K), six aspects; $0.05 / $0.08 / $0.13 / $0.16 a second; "The
///   first five reference images are free; each one after that adds US $0.04".
/// - Seedance 2.0: prompt ≤ 2,000, duration 4–15, 480p | 720p, six aspects or
///   auto; $0.07 / $0.16 a second.
/// - Seedance 2.5: prompt ≤ 30,000, duration 4–30, 480p | 720p | 1080p;
///   $0.103 / $0.231 / $0.614 a second.
///
/// Kling 4.0 is priced but has no endpoint in the spec or the docs: not carried.
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-10)
/// @see <https://ideogram.ai/api/usage-pricing/catalog> (2026-11-10)
#[test]
fn ideogram_video_resales_are_carried_with_ideograms_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    // (id, prompt max, aspects, resolutions, duration (min, max), references, audio, base)
    for (id, prompt_max, aspects, resolutions, (min, max), references, audio, base) in [
        ("ideogram/kling-3-standard", 2000, &["16:9", "9:16", "1:1"][..], &[][..], (3, 15), 0, true, 0.42),
        ("ideogram/minimax-h3", 2000, &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"][..], &["480P", "768P", "2K", "4K"][..], (5, 15), 9, false, 0.65),
        ("ideogram/seedance-2", 2000, &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"][..], &["480p", "720p"][..], (4, 15), 9, true, 0.8),
        ("ideogram/seedance-2-5", 30000, &["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"][..], &["480p", "720p", "1080p"][..], (4, 30), 9, true, 1.155),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.media_type, litegen::capabilities::MediaType::Video, "{id}");
        assert!(m.capabilities.text_to_video && m.capabilities.image_to_video, "{id}");
        assert!(m.docs_url.as_deref().is_some_and(|u| u.starts_with("https://developer.ideogram.ai/v2/api-reference/video/generate/")), "{id}: {:?}", m.docs_url);
        assert_eq!(m.prompt.max_length, Some(prompt_max), "{id}");
        assert_eq!(aspect_ratios(&reg, id), aspects, "{id}");
        assert_eq!(enum_values(&reg, id, "resolution"), resolutions, "{id}");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(min), Some(max), Some(5)), "{id}"),
            other => panic!("{id} duration_seconds: {other:?}"),
        }
        assert_eq!(m.params.contains_key("audio"), audio, "{id}: generate_audio");
        let roles = &m.ref_inputs.as_ref().unwrap().roles;
        assert_eq!((roles["first_frame"].max_count, roles["last_frame"].max_count), (1, 1), "{id}: image and end_image");
        assert_eq!(roles.get("reference").map_or(0, |r| r.max_count), references, "{id}: reference_images");
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: {} (want {base})", m.pricing.base_cost_usd);
        assert!(!m.extra_allowlist.iter().any(|k| k == "webhook_url"), "{id}: litegen fires its own webhooks");
    }
    let price = |id: &str, params: serde_json::Value, references: usize| {
        let m = reg.get(id).unwrap();
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        req.as_object_mut().unwrap().extend(params.as_object().unwrap().clone());
        req["reference_images"] = (0..references).map(|i| serde_json::json!({ "type": "url", "value": format!("https://e.com/{i}.png"), "role": "reference" })).collect();
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    use serde_json::json as j;
    for (id, params, references, usd) in [
        ("ideogram/kling-3-standard", j!({ "audio": true, "duration_seconds": 10 }), 0, 1.26),
        ("ideogram/minimax-h3", j!({ "resolution": "480P" }), 0, 0.25),
        ("ideogram/minimax-h3", j!({ "resolution": "4K", "duration_seconds": 10 }), 7, 1.68),
        ("ideogram/minimax-h3", j!({}), 5, 0.65),
        ("ideogram/seedance-2", j!({ "resolution": "480p", "duration_seconds": 15 }), 0, 1.05),
        ("ideogram/seedance-2-5", j!({ "resolution": "1080p", "duration_seconds": 30 }), 0, 18.42),
    ] {
        let got = price(id, params.clone(), references);
        assert!((got - usd).abs() < 1e-9, "{id} {params} + {references} refs: {got} (want {usd})");
    }
}

/// Nano Banana resold on Ideogram v2 (`/v2/image/generate/nano-banana-2` and
/// `nano-banana-pro`, multipart): "images … max 10", aspect_ratio "auto","1:1",…
/// (2: + "4:1","1:4","8:1","1:8"), "resolution_tier … 1K, 2K, 4K". Ideogram's
/// price (rendered): "Nano Banana 2 Generate 1K | US $0.0672", 2K $0.1008, 4K
/// $0.1512; "Nano Banana Pro Generate 1K | US $0.1344", 2K $0.1344, 4K $0.24.
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-03)
/// @see <https://ideogram.ai/pricing/?pricing_tab=api> (rendered 2026-11-03)
#[test]
fn ideogram_resold_nano_banana_rows_are_carried() {
    let reg = registry();
    for (id, base, four_k, ratios) in [("ideogram/nano-banana-2", 0.0672, 0.1512, 14), ("ideogram/nano-banana-pro", 0.1344, 0.24, 10)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - base).abs() < 1e-9, "{id}: 1K ${base}");
        assert!(m.pricing.variable_pricing.iter().any(|r| r.when.get("resolution") == Some(&serde_json::json!("4K")) && r.usd.is_some_and(|u| (u - four_k).abs() < 1e-9)), "{id}: 4K ${four_k}");
        assert_eq!(enum_values(&reg, id, "resolution"), ["1K", "2K", "4K"], "{id}");
        assert_eq!(aspect_ratios(&reg, id).len(), ratios, "{id}");
        assert_eq!(m.ref_inputs.as_ref().map(|r| r.max_total), Some(10), "{id}");
    }
}

/// SD 3.5 on Replicate (official models, versionless): inputs prompt,
/// aspect_ratio ("16:9", "1:1", "21:9", "2:3", "3:2", "4:5", "5:4", "9:16",
/// "9:21"), cfg (1-10; Large and Medium default 5, Large Turbo 1), seed,
/// image, output_format (webp, jpg, png), negative_prompt, prompt_strength
/// (0-1, default 0.85). Price "per output image": $0.065, $0.04, $0.035.
/// @see <https://replicate.com/stability-ai/stable-diffusion-3.5-large/llms.txt> (2026-11-04)
#[test]
fn replicate_sd3_5_rows_are_carried_with_their_documented_contract() {
    let reg = registry();
    for (id, usd, cfg_default) in [("replicate/sd3.5-large", 0.065, 5.0), ("replicate/sd3.5-large-turbo", 0.04, 1.0), ("replicate/sd3.5-medium", 0.035, 5.0)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id}");
        assert!(m.capabilities.text_to_image && m.capabilities.image_to_image, "{id}");
        match m.params.get("guidance_scale") {
            Some(ParamSpec::Float(g)) => assert_eq!((g.min, g.max, g.default), (Some(1.0), Some(10.0), Some(cfg_default)), "{id}"),
            other => panic!("{id}: {other:?}"),
        }
        assert!(!m.params.contains_key("steps"), "{id}: no steps input");
        assert_eq!(aspect_ratios(&reg, id), ["16:9", "1:1", "21:9", "2:3", "3:2", "4:5", "5:4", "9:16", "9:21"], "{id}");
        assert_eq!(enum_values(&reg, id, "output_format"), ["webp", "jpeg", "png"], "{id}");
    }
}

/// FLUX 3 on fal. FLUX 3 Image (`blackforestlabs/flux-3/text-to-image`,
/// `/edit-image`): resolution "768sq" … "4k", aspect_ratio "auto" + 14,
/// output_format jpeg | png, edits "image_urls … maxItems 10"; fal prices it only
/// at 1K ("A 1K (1 MP) image costs $0.048"), so the row offers 1K. Klein
/// (`…/klein/text-to-image`, `/klein/edit-image`): image_size "Each side can be
/// 256 to 4096 pixels" (multiple_of 16, max_area 16777216), seed, output_format
/// jpeg | png | webp; "$0.008 at 768 × 768, $0.009 at 1K, $0.011 at 1.5K, $0.014
/// at 2K, and $0.036 at 4K" by output pixels.
/// @see <https://fal.ai/models/blackforestlabs/flux-3/text-to-image/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/blackforestlabs/flux-3/klein/text-to-image/llms.txt> (2026-11-04)
#[test]
fn fal_flux_3_rows_are_carried_with_fals_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let image = reg.get("fal/flux-3-image").expect("fal/flux-3-image missing");
    assert!((image.pricing.base_cost_usd - 0.048).abs() < 1e-9);
    assert_eq!(enum_values(&reg, "fal/flux-3-image", "resolution"), ["1K"], "only 1K is priced");
    assert_eq!(enum_values(&reg, "fal/flux-3-image", "output_format"), ["jpeg", "png"]);
    assert!(!image.params.contains_key("size") && !image.params.contains_key("seed"));
    let klein = reg.get("fal/flux-3-image-klein").expect("fal/flux-3-image-klein missing");
    assert!((klein.pricing.base_cost_usd - 0.009).abs() < 1e-9, "square_hd (1024²) by default");
    match klein.params.get("size") {
        Some(ParamSpec::Size(SizeSpec::Freeform(s))) => {
            assert_eq!((s.min_width, s.max_width, s.multiple_of, s.max_pixels), (256, 4096, Some(16), Some(16_777_216)));
        }
        other => panic!("klein size: {other:?}"),
    }
    assert!(!klein.params.contains_key("resolution"));
    assert_eq!(enum_values(&reg, "fal/flux-3-image-klein", "output_format"), ["png", "jpeg", "webp"]);
    assert!(klein.pricing.variable_pricing.iter().any(|r| r.when.get("megapixels") == Some(&serde_json::json!({ "gt": 4.194304 })) && r.usd.is_some_and(|u| (u - 0.036).abs() < 1e-9)));
    for id in ["fal/flux-3-image", "fal/flux-3-image-klein"] {
        assert_eq!(reg.get(id).and_then(|m| m.ref_inputs.as_ref()).map(|r| r.max_total), Some(10), "{id}");
    }
}

/// FLUX 3 Klein *base* on fal ("undistilled, CFG"), a model of its own, which
/// fal serves only at `…/klein/base/text-to-image/lora` and
/// `…/klein/base/edit-image/lora`. `loras` is optional ("Default: `[]`"), so
/// the row calls them with none. "Same inputs as the text-to-image endpoint":
/// image_size 256 to 4096 a side (multiple_of 16), seed, output_format jpeg |
/// png | webp, edits from at most 10 `image_urls`. "$0.035 per megapixel of
/// output … up to 1024x1024 counts as 1 megapixel, 2048x2048 as 1.56", so
/// $0.035 up to 1 MP and $0.0546 (1.56 × $0.035) above it. fal prices nothing
/// past 2048², so the row stops at 4,194,304 pixels.
/// @see <https://fal.ai/models/blackforestlabs/flux-3/klein/base/text-to-image/lora/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/blackforestlabs/flux-3/klein/base/edit-image/lora/llms.txt> (2026-11-04)
#[test]
fn fal_flux_3_klein_base_is_carried_with_fals_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let id = "fal/flux-3-image-klein-base";
    let m = reg.get(id).expect("fal/flux-3-image-klein-base missing");
    assert_eq!(m.docs_url.as_deref(), Some("https://fal.ai/models/blackforestlabs/flux-3/klein/base/text-to-image/lora/api"));
    match m.params.get("size") {
        Some(ParamSpec::Size(SizeSpec::Freeform(s))) => {
            assert_eq!(
                (s.min_width, s.max_width, s.min_height, s.max_height, s.multiple_of, s.max_pixels),
                (256, 4096, 256, 4096, Some(16), Some(4_194_304)),
                "fal prices sizes only up to 2048²"
            );
        }
        other => panic!("klein base size: {other:?}"),
    }
    assert_eq!(enum_values(&reg, id, "output_format"), ["png", "jpeg", "webp"]);
    assert!(m.params.contains_key("seed") && !m.params.contains_key("resolution"));
    assert_eq!(m.ref_inputs.as_ref().map(|r| r.max_total), Some(10));
    assert!(!m.extra_allowlist.iter().any(|k| k == "loras"), "litegen takes no LoRA input: {:?}", m.extra_allowlist);
    let price = |size: Option<&str>| {
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        if let Some(s) = size {
            req["size"] = s.into();
        }
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    for (size, usd) in [(None, 0.035), (Some("768x768"), 0.035), (Some("1024x1024"), 0.035), (Some("1536x1536"), 0.0546), (Some("2048x2048"), 0.0546), (Some("4096x1024"), 0.0546)] {
        assert!((price(size) - usd).abs() < 1e-9, "{size:?}: {} (want {usd})", price(size));
    }
}

/// Recraft V4 and V4.1 on fal, carried as fal routes beside recraft/* (the
/// hub-route policy, 2026-11-04). Each takes a prompt up to 10,000 characters,
/// `image_size` and `colors`, and has no seed or style. The 1K models (standard,
/// vector, utility, flash) use Recraft's 1K size table and the pro models its
/// 2K one (Recraft's appendix, as on recraft/recraftv4_1 and _pro). Prices
/// "per images": V4 $0.04, pro $0.25, vector $0.08, pro vector $0.30; V4.1
/// $0.035, pro $0.21, vector $0.08, pro vector $0.30, utility $0.035, utility
/// pro $0.21, flash $0.007.
/// @see <https://fal.ai/models/fal-ai/recraft/v4/text-to-image/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/recraft/v4.1/pro/text-to-vector/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/recraft/v4.1/flash/text-to-image/llms.txt> (2026-11-04)
/// @see <https://www.recraft.ai/docs/api-reference/appendix.md> (2026-11-03)
#[test]
fn fal_recraft_v4_rows_are_carried_with_fals_contract() {
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    let one_k: Vec<(u32, u32)> = vec![(1024, 1024), (1536, 768), (768, 1536), (1280, 832), (832, 1280), (1216, 896), (896, 1216), (1152, 896), (896, 1152), (832, 1344), (1280, 896), (896, 1280), (1344, 768), (768, 1344)];
    let two_k: Vec<(u32, u32)> = vec![(2048, 2048), (3072, 1536), (1536, 3072), (2560, 1664), (1664, 2560), (2432, 1792), (1792, 2432), (2304, 1792), (1792, 2304), (1664, 2688), (2560, 1792), (1792, 2560), (2688, 1536), (1536, 2688)];
    for (id, usd, pro, vector) in [
        ("fal/recraft-v4", 0.04, false, false),
        ("fal/recraft-v4-pro", 0.25, true, false),
        ("fal/recraft-v4-vector", 0.08, false, true),
        ("fal/recraft-v4-pro-vector", 0.30, true, true),
        ("fal/recraft-v4.1", 0.035, false, false),
        ("fal/recraft-v4.1-pro", 0.21, true, false),
        ("fal/recraft-v4.1-vector", 0.08, false, true),
        ("fal/recraft-v4.1-pro-vector", 0.30, true, true),
        ("fal/recraft-v4.1-utility", 0.035, false, false),
        ("fal/recraft-v4.1-utility-pro", 0.21, true, false),
        ("fal/recraft-v4.1-flash", 0.007, false, false),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id}: {}", m.pricing.base_cost_usd);
        assert!(m.capabilities.text_to_image && !m.capabilities.image_to_image, "{id}");
        assert_eq!(m.prompt.max_length, Some(10000), "{id}");
        assert!(m.docs_url.as_deref().is_some_and(|u| u.starts_with("https://fal.ai/models/") && u.contains("recraft/v4")), "{id}: {:?}", m.docs_url);
        match m.params.get("size") {
            Some(ParamSpec::Size(SizeSpec::Enum(s))) => {
                assert_eq!(s.values, if pro { two_k.clone() } else { one_k.clone() }, "{id}: Recraft's {} table", if pro { "2K" } else { "1K" });
            }
            other => panic!("{id} size: {other:?}"),
        }
        assert!(matches!(m.params.get("palette"), Some(ParamSpec::Palette(p)) if p.max_colors == 10 && !p.forced), "{id}: palette as fal's colors");
        assert!(!m.params.contains_key("seed") && !m.params.contains_key("style"), "{id}: fal takes neither");
        assert_eq!(m.tags.iter().any(|t| t == "vector"), vector, "{id}");
    }
    // Recraft V2 (fal-ai/recraft-20b 308-redirects to fal-ai/recraft/v2/text-to-image):
    // "$0.022 per image (or $0.044 if you are using a vector style)", prompt maxLength
    // 1000, the three base styles, as fal/recraft-v3.
    let v2 = reg.get("fal/recraft-v2").expect("fal/recraft-v2 missing");
    assert!((v2.pricing.base_cost_usd - 0.022).abs() < 1e-9);
    assert!(v2.pricing.variable_pricing.iter().any(|r| r.when.get("style") == Some(&serde_json::json!("vector_illustration")) && r.multiply == Some(2.0)));
    assert_eq!(enum_values(&reg, "fal/recraft-v2", "style"), ["realistic_image", "digital_illustration", "vector_illustration"]);
    assert_eq!(v2.prompt.max_length, Some(1000));
    assert!(matches!(v2.params.get("palette"), Some(ParamSpec::Palette(p)) if p.max_colors == 10));
}

/// The rest of Recraft on fal (2026-11-04):
/// - V3 takes an image through its `/image-to-image` twin ("$0.04 per image (or
///   $0.08 if you are using a vector style)"), with `strength` 0-1 (default
///   0.5) and a negative prompt there only.
/// - V4 style references, `recraft/v4/style/{,pro/}text-to-{image,vector}`:
///   up to 10 `image_urls` as `style` references, at "$0.035", "$0.10",
///   "$0.05" and "$0.12" with a style_id, plus "$0.005 extra" when the style is
///   made from the images.
/// - vectorize "$0.01", upscale crisp "$0.004" and creative "$0.25 per
///   images": one image in, one out, no prompt.
///
/// @see <https://fal.ai/models/fal-ai/recraft/v3/image-to-image/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/recraft/v4/style/text-to-image/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/recraft/vectorize/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/recraft/upscale/creative/llms.txt> (2026-11-04)
#[test]
fn fal_recraft_operations_are_carried_with_fals_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    let v3 = reg.get("fal/recraft-v3").expect("fal/recraft-v3 missing");
    assert!(v3.capabilities.text_to_image && v3.capabilities.image_to_image, "V3: text, or an image through its twin");
    assert_eq!(v3.ref_inputs.as_ref().and_then(|r| r.roles.get("init")).map(|r| (r.required, r.max_count)), Some((false, 1)));
    assert!(matches!(v3.params.get("strength"), Some(ParamSpec::Float(s)) if (s.min, s.max, s.default) == (Some(0.0), Some(1.0), Some(0.5))));
    let price = |id: &str, refs: usize, extra: Option<serde_json::Value>| {
        let m = reg.get(id).unwrap();
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        if refs > 0 {
            req["reference_images"] = (0..refs).map(|i| serde_json::json!({ "type": "url", "value": format!("https://cdn.example/{i}.png"), "role": "style" })).collect();
        }
        if let Some(e) = extra {
            req["extra"] = e;
        }
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    for (id, usd) in [("fal/recraft-v4-style", 0.035), ("fal/recraft-v4-style-pro", 0.10), ("fal/recraft-v4-style-vector", 0.05), ("fal/recraft-v4-style-pro-vector", 0.12)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id}");
        assert_eq!(m.ref_inputs.as_ref().and_then(|r| r.roles.get("style")).map(|r| (r.required, r.max_count)), Some((false, 10)), "{id}: style references");
        assert!(m.extra_allowlist.iter().any(|k| k == "style_id"), "{id}");
        assert!((price(id, 2, None) - (usd + 0.005)).abs() < 1e-9, "{id}: a style made from the images");
        assert!((price(id, 0, Some(serde_json::json!({ "style_id": "s-1" }))) - usd).abs() < 1e-9, "{id}: a given style_id");
    }
    for (id, usd) in [("fal/recraft-vectorize", 0.01), ("fal/recraft-upscale-crisp", 0.004), ("fal/recraft-upscale-creative", 0.25)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id}");
        assert!(m.capabilities.image_to_image && !m.capabilities.text_to_image, "{id}");
        assert_eq!(m.prompt.max_length, Some(0), "{id}: no prompt");
        assert_eq!(m.ref_inputs.as_ref().and_then(|r| r.roles.get("init")).map(|r| (r.required, r.max_count)), Some((true, 1)), "{id}");
    }
}

/// FLUX.1 [pro] on fal, carried as fal routes beside bfl/flux-kontext-* and
/// bfl/flux-pro-1.1-ultra (the hub-route policy, 2026-11-04):
/// - Kontext [pro] and [max]: "$0.04" and "$0.08 per images". No image goes to
///   `…/kontext[/max]/text-to-image`, one to `…/kontext[/max]` (`image_url`),
///   and 2 to 4 to `…/multi` (`image_urls`; BFL: `input_image` … `input_image_4`).
/// - 1.1 [pro] ultra: "$0.06 per images", with an optional image prompt
///   (`image_url`).
/// - Redux on 1.1 [pro] and on ultra: "$0.05 per megapixels"; ultra's output
///   is BFL's "4MP", billed as its 2752 × 1536.
/// - Fill: "$0.05 per megapixels", a mask where "white areas (100%) specify
///   areas for inpainting" (BFL).
/// - Erase: "$0.03 for the first megapixel generated … Each reference megapixel
///   also costs $0.004 set at 3 MP minimum", so $0.042 at least; its mask's
///   "white (255) marks pixels to erase", and it takes no prompt.
///
/// Fill and erase take a `region` too, drawn as their mask, as every
/// mask-taking row does.
/// @see <https://fal.ai/models/fal-ai/flux-pro/kontext/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-pro/v1.1-ultra/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-pro/v1.1/redux/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-pro/v1/fill/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-pro/v1/erase/llms.txt> (2026-11-04)
/// @see <https://api.bfl.ai/openapi.json> (2026-11-04)
#[test]
fn fal_flux_1_pro_rows_are_carried_with_fals_contract() {
    use litegen::capabilities::MaskConvention;
    let reg = registry();
    let kontext_ratios = ["21:9", "16:9", "4:3", "3:2", "1:1", "2:3", "3:4", "9:16", "9:21"];
    for (id, usd) in [("fal/flux-kontext-pro", 0.04), ("fal/flux-kontext-max", 0.08)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id}");
        assert!(m.capabilities.text_to_image && m.capabilities.image_to_image, "{id}");
        assert_eq!(m.ref_inputs.as_ref().map(|r| r.max_total), Some(4), "{id}: input_image … input_image_4");
        assert_eq!(aspect_ratios(&reg, id), kontext_ratios, "{id}");
        assert!(matches!(m.params.get("guidance_scale"), Some(ParamSpec::Float(g)) if (g.min, g.max, g.default) == (Some(1.0), Some(20.0), Some(3.5))), "{id}");
    }
    let ultra = reg.get("fal/flux-pro-1.1-ultra").expect("fal/flux-pro-1.1-ultra missing");
    assert!((ultra.pricing.base_cost_usd - 0.06).abs() < 1e-9);
    assert_eq!(ultra.ref_inputs.as_ref().and_then(|r| r.roles.get("redux")).map(|r| (r.required, r.max_count)), Some((false, 1)), "an optional image prompt");
    assert_eq!(aspect_ratios(&reg, "fal/flux-pro-1.1-ultra"), kontext_ratios);
    for (id, usd) in [("fal/flux-pro-1.1-redux", 0.05), ("fal/flux-pro-1.1-ultra-redux", 0.211_353_6)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id}: {}", m.pricing.base_cost_usd);
        assert!(m.capabilities.image_to_image && !m.capabilities.text_to_image && !m.prompt.required, "{id}: a variation of one image, the prompt optional");
        assert_eq!(m.ref_inputs.as_ref().and_then(|r| r.roles.get("init")).map(|r| (r.required, r.max_count)), Some((true, 1)), "{id}");
    }
    for (id, usd, prompt) in [("fal/flux-pro-fill", 0.05, true), ("fal/flux-pro-erase", 0.042, false)] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!((m.pricing.base_cost_usd - usd).abs() < 1e-9, "{id}");
        assert!(m.capabilities.inpainting, "{id}");
        let roles = &m.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} refs")).roles;
        assert_eq!(roles.get("init").map(|r| (r.required, r.max_count)), Some((true, 1)), "{id}");
        assert_eq!(roles.get("mask").map(|r| (r.max_count, r.convention)), Some((1, Some(MaskConvention::WhiteEdits))), "{id}");
        assert!(matches!(m.params.get("region"), Some(ParamSpec::Region(_))), "{id}: a region, drawn as the mask");
        assert_eq!(m.prompt.required, prompt, "{id}");
        assert_eq!(m.prompt.max_length == Some(0), !prompt, "{id}: erase takes no prompt");
    }
}

/// FLUX.2 [pro] outpaint on fal (`fal-ai/flux-2-pro/outpaint`): the init image
/// and "Pixels to expand on the left of the image" (`expand_left`, `_top`,
/// `_right`, `_bottom`, each 0 to 2048), with mode high | fast and auto_crop,
/// and no prompt. "$0.03 for the first megapixel of output, plus $0.015 per
/// extra megapixel of input and output": $0.045, one megapixel in and one out,
/// is the least an outpaint costs.
/// @see <https://fal.ai/models/fal-ai/flux-2-pro/outpaint/llms.txt> (2026-11-04)
#[test]
fn fal_flux_2_pro_outpaint_is_carried_with_fals_contract() {
    let reg = registry();
    let id = "fal/flux-2-pro-outpaint";
    let m = reg.get(id).expect("fal/flux-2-pro-outpaint missing");
    assert_eq!(m.docs_url.as_deref(), Some("https://fal.ai/models/fal-ai/flux-2-pro/outpaint/api"));
    assert!((m.pricing.base_cost_usd - 0.045).abs() < 1e-9);
    assert!(m.capabilities.image_to_image && !m.capabilities.text_to_image);
    match m.params.get("expand") {
        Some(ParamSpec::Expand(e)) => assert_eq!((e.max_px, e.centered_only), (Some(2048), false)),
        other => panic!("{id} expand: {other:?}"),
    }
    assert_eq!(m.prompt.max_length, Some(0), "the endpoint takes no prompt");
    let init = m.ref_inputs.as_ref().and_then(|r| r.roles.get("init")).map(|r| (r.required, r.min_count, r.max_count));
    assert_eq!(init, Some((true, 1, 1)));
    assert_eq!(enum_values(&reg, id, "output_format"), ["jpeg", "png"]);
    assert_eq!(m.extra_allowlist, ["mode", "auto_crop"]);
}

/// FLUX.2 on fal, carried as fal routes beside bfl/flux-2-* (the hub-route
/// policy, 2026-11-04). Each row is a text-to-image endpoint plus an `/edit`
/// twin that takes the images as `image_urls`.
/// - dev, turbo, flash (`fal-ai/flux-2[/turbo|/flash]`): "$0.012", "$0.008"
///   and "$0.005 per megapixel of input and output. Input images will be
///   resized to 1MP", with "A maximum of 4 images".
/// - [pro] (`fal-ai/flux-2-pro`): "$0.03 for the first output megapixel, plus
///   $0.015 per additional input or output megapixel, with megapixels rounded
///   up … 1 megapixel is 1024 × 1024 pixels".
/// - [max]: "$0.07. Each additional megapixel will cost $0.03. Input images
///   counted as processed megapixel".
/// - [flex]: "$0.05 per megapixel on both input and output side, rounded up".
///   An input counts at least one megapixel.
/// - [klein] 4B and 9B (`fal-ai/flux-2/klein/{4b,9b}`): "$0.005" and "$0.006"
///   per megapixel, their edits "$0.01" and "$0.011" per megapixel of input
///   and output. The base models are "$0.009" and "$0.011". All take at most
///   4 images.
///
/// Sides run 256 to 2048, BFL's FLUX.2 limit (fal states none for these, and
/// its own 1920×1080 price example is no multiple of 32), and 512 to 2048 on
/// dev, turbo and flash ("The width and height must be between 512 and 2048
/// pixels"). The 4 MP price bands cover that range.
/// @see <https://fal.ai/models/fal-ai/flux-2/edit/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-2-pro/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-2-max/edit/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-2-flex/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-2/klein/9b/edit/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/fal-ai/flux-2/klein/4b/base/edit/llms.txt> (2026-11-04)
#[test]
fn fal_flux_2_rows_are_carried_with_fals_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    use litegen::capabilities::schema::SizeSpec;
    let reg = registry();
    /// One row's facts: the images it takes, its smallest side, its output
    /// formats, guidance and steps as (min, max, default), and whether it
    /// takes a negative prompt.
    struct Row {
        id: &'static str,
        images: u32,
        min_side: u32,
        formats: &'static [&'static str],
        guidance: Option<(f64, f64, f64)>,
        steps: Option<(i64, i64, i64)>,
        negative: bool,
    }
    const DEV: &[&str] = &["png", "webp", "jpeg"];
    const PRO: &[&str] = &["jpeg", "png"];
    let rows = [
        Row { id: "fal/flux-2", images: 4, min_side: 512, formats: DEV, guidance: Some((0.0, 20.0, 2.5)), steps: Some((4, 50, 28)), negative: false },
        Row { id: "fal/flux-2-turbo", images: 4, min_side: 512, formats: DEV, guidance: Some((0.0, 20.0, 2.5)), steps: None, negative: false },
        Row { id: "fal/flux-2-flash", images: 4, min_side: 512, formats: DEV, guidance: Some((0.0, 20.0, 2.5)), steps: None, negative: false },
        Row { id: "fal/flux-2-pro", images: 8, min_side: 256, formats: PRO, guidance: None, steps: None, negative: false },
        Row { id: "fal/flux-2-max", images: 8, min_side: 256, formats: PRO, guidance: None, steps: None, negative: false },
        Row { id: "fal/flux-2-flex", images: 8, min_side: 256, formats: PRO, guidance: Some((1.5, 10.0, 3.5)), steps: Some((2, 50, 28)), negative: false },
        Row { id: "fal/flux-2-klein-4b", images: 4, min_side: 256, formats: DEV, guidance: None, steps: Some((4, 8, 4)), negative: false },
        Row { id: "fal/flux-2-klein-9b", images: 4, min_side: 256, formats: DEV, guidance: None, steps: Some((4, 8, 4)), negative: false },
        Row { id: "fal/flux-2-klein-4b-base", images: 4, min_side: 256, formats: DEV, guidance: Some((0.0, 20.0, 5.0)), steps: Some((4, 50, 28)), negative: true },
        Row { id: "fal/flux-2-klein-9b-base", images: 4, min_side: 256, formats: DEV, guidance: Some((0.0, 20.0, 5.0)), steps: Some((4, 50, 28)), negative: true },
    ];
    for Row { id, images, min_side, formats, guidance, steps, negative } in rows {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(m.capabilities.text_to_image && m.capabilities.image_to_image, "{id}: text, and edits from images");
        assert!(m.docs_url.as_deref().is_some_and(|u| u.starts_with("https://fal.ai/models/fal-ai/flux-2")), "{id}: {:?}", m.docs_url);
        assert_eq!(m.ref_inputs.as_ref().map(|r| (r.max_total, r.roles.get("init").map(|i| i.max_count))), Some((images, Some(images))), "{id}");
        match m.params.get("size") {
            Some(ParamSpec::Size(SizeSpec::Freeform(s))) => {
                assert_eq!((s.min_width, s.max_width, s.min_height, s.max_height, s.multiple_of), (min_side, 2048, min_side, 2048, None), "{id}");
            }
            other => panic!("{id} size: {other:?}"),
        }
        assert_eq!(enum_values(&reg, id, "output_format"), formats, "{id}");
        match (m.params.get("guidance_scale"), guidance) {
            (Some(ParamSpec::Float(g)), Some((lo, hi, d))) => assert_eq!((g.min, g.max, g.default), (Some(lo), Some(hi), Some(d)), "{id} guidance"),
            (None, None) => {}
            (got, want) => panic!("{id} guidance_scale: {got:?}, want {want:?}"),
        }
        match (m.params.get("steps"), steps) {
            (Some(ParamSpec::Int(s)), Some((lo, hi, d))) => assert_eq!((s.min, s.max, s.default), (Some(lo), Some(hi), Some(d)), "{id} steps"),
            (None, None) => {}
            (got, want) => panic!("{id} steps: {got:?}, want {want:?}"),
        }
        assert_eq!(m.params.contains_key("negative_prompt"), negative, "{id}: only the klein base models take a negative prompt");
        assert!(m.params.contains_key("seed"), "{id}");
    }
    let price = |id: &str, size: Option<&str>, refs: usize| {
        let m = reg.get(id).unwrap();
        let mut req = serde_json::json!({ "model": id, "prompt": "x" });
        if let Some(s) = size {
            req["size"] = s.into();
        }
        if refs > 0 {
            req["reference_images"] = (0..refs).map(|i| serde_json::json!({ "type": "url", "value": format!("https://cdn.example/{i}.png"), "role": "init" })).collect();
        }
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    for (id, size, refs, usd) in [
        ("fal/flux-2", None, 0, 0.012),
        ("fal/flux-2", None, 1, 0.024),
        ("fal/flux-2-turbo", None, 1, 0.016),
        ("fal/flux-2-flash", None, 0, 0.005),
        ("fal/flux-2-flash", None, 2, 0.015),
        ("fal/flux-2-pro", None, 0, 0.03),
        ("fal/flux-2-pro", Some("1920x1080"), 0, 0.045),
        ("fal/flux-2-pro", Some("2048x2048"), 0, 0.075),
        ("fal/flux-2-pro", Some("1920x1080"), 2, 0.075),
        ("fal/flux-2-max", None, 0, 0.07),
        ("fal/flux-2-max", Some("1920x1080"), 1, 0.13),
        ("fal/flux-2-flex", Some("1920x1080"), 0, 0.10),
        ("fal/flux-2-flex", None, 1, 0.10),
        ("fal/flux-2-klein-4b", None, 0, 0.005),
        ("fal/flux-2-klein-4b", None, 1, 0.02),
        ("fal/flux-2-klein-9b", None, 0, 0.006),
        ("fal/flux-2-klein-9b", None, 1, 0.022),
        ("fal/flux-2-klein-4b-base", None, 1, 0.018),
        ("fal/flux-2-klein-9b-base", None, 0, 0.011),
        ("fal/flux-2-klein-9b-base", None, 1, 0.022),
    ] {
        let got = price(id, size, refs);
        assert!((got - usd).abs() < 1e-9, "{id} size {size:?} with {refs} images: {got} (want {usd})");
    }
}

/// FLUX 3 video on fal: three rows, each reached four ways (text, a first
/// frame, a first and a last frame, keyframes).
/// - `fal/flux-3-video`: `blackforestlabs/flux-3/{text,image,first-last-frame,keyframes}-to-video`,
///   5 to 20 s, 720p to 2160p. "$0.17 per second of generated video at 720p,
///   $0.29 per second at 1080p, $0.32 per second at 2K (1440p), and $0.47 per
///   second at 4K (2160p)". The first-last and keyframe endpoints price 720p and
///   1080p only, and the adapter refuses the rest there.
/// - `fal/flux-3-video-draft`: the four `/draft` twins, 720p with no resolution
///   field. "$0.06 per second of generated draft video (720p)".
/// - `fal/flux-3-max-video`: `blackforestlabs/flux-3-max/{text,image,keyframes}-to-video`,
///   720p | 1080p, a seed, and the last frame as image-to-video's
///   `end_image_url`. It bills the list price, "after which 720p is
///   $0.17/second and 1080p is $0.29/second", because the 75 % off promotion
///   ends 2026-11-07.
///
/// Keyframes are "a position at 24 fps", at most 10.
/// @see <https://fal.ai/models/blackforestlabs/flux-3/text-to-video/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/blackforestlabs/flux-3/first-last-frame-to-video/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/blackforestlabs/flux-3/text-to-video/draft/llms.txt> (2026-11-04)
/// @see <https://fal.ai/models/blackforestlabs/flux-3-max/text-to-video/llms.txt> (2026-11-04)
/// @see <https://docs.bfl.ml/cookbook/video_start_from_images.md> (2026-11-04)
#[test]
fn fal_flux_3_video_rows_are_carried_with_fals_contract() {
    use litegen::capabilities::pricing::{unit_price_usd, PriceFacts};
    let reg = registry();
    let ratios = ["21:9", "2:1", "16:9", "4:3", "1:1", "3:4", "9:16"];
    for (id, resolutions, extra, docs) in [
        ("fal/flux-3-video", vec!["720p", "1080p", "1440p", "2160p"], "safety_tolerance", "https://fal.ai/models/blackforestlabs/flux-3/text-to-video/api"),
        ("fal/flux-3-video-draft", vec![], "safety_tolerance", "https://fal.ai/models/blackforestlabs/flux-3/text-to-video/draft/api"),
        ("fal/flux-3-max-video", vec!["720p", "1080p"], "reasoning_level", "https://fal.ai/models/blackforestlabs/flux-3-max/text-to-video/api"),
    ] {
        let m = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(m.docs_url.as_deref(), Some(docs), "{id}");
        assert!(m.capabilities.text_to_video && m.capabilities.image_to_video, "{id}");
        match m.params.get("duration_seconds") {
            Some(ParamSpec::Int(d)) => assert_eq!((d.min, d.max, d.default), (Some(5), Some(20), Some(5)), "{id}"),
            other => panic!("{id} duration_seconds: {other:?}"),
        }
        assert_eq!(enum_values(&reg, id, "resolution"), resolutions, "{id}");
        assert_eq!(aspect_ratios(&reg, id), ratios, "{id}");
        assert!(matches!(m.params.get("audio"), Some(ParamSpec::Bool(b)) if b.default == Some(true)), "{id}: audio on by default");
        let refs = m.ref_inputs.as_ref().unwrap_or_else(|| panic!("{id} has no ref_inputs"));
        assert_eq!(refs.roles.get("keyframe").map(|r| r.max_count), Some(10), "{id}");
        for role in ["first_frame", "last_frame"] {
            assert_eq!(refs.roles.get(role).map(|r| (r.required, r.max_count)), Some((false, 1)), "{id} {role}");
        }
        assert_eq!(m.extra_allowlist, [extra], "{id}");
        assert_eq!(m.params.contains_key("seed"), id == "fal/flux-3-max-video", "{id}: only Max takes a seed");
    }
    let price = |id: &str, mut req: serde_json::Value| {
        let m = reg.get(id).unwrap();
        req["model"] = id.into();
        req["prompt"] = "x".into();
        let base: litegen::types::BaseGenerationRequest = serde_json::from_value(req.clone()).unwrap();
        unit_price_usd(m, &PriceFacts::new(m, &base, &req))
    };
    for (id, req, usd) in [
        ("fal/flux-3-video", serde_json::json!({}), 0.85),
        ("fal/flux-3-video", serde_json::json!({ "duration_seconds": 10, "resolution": "1080p" }), 2.9),
        ("fal/flux-3-video", serde_json::json!({ "resolution": "1440p" }), 1.6),
        ("fal/flux-3-video", serde_json::json!({ "duration_seconds": 20, "resolution": "2160p" }), 9.4),
        ("fal/flux-3-video-draft", serde_json::json!({}), 0.3),
        ("fal/flux-3-video-draft", serde_json::json!({ "duration_seconds": 20 }), 1.2),
        ("fal/flux-3-max-video", serde_json::json!({}), 0.85),
        ("fal/flux-3-max-video", serde_json::json!({ "duration_seconds": 8, "resolution": "1080p" }), 2.32),
    ] {
        let got = price(id, req.clone());
        assert!((got - usd).abs() < 1e-9, "{id} {req}: {got} (want {usd})");
    }
}

// ─── 2026-11-09 provider audit (docs/superpowers/2026-11-09-provider-audit.md) ───

/// Ideogram 4.5's v2 schema caps the prompt: `prompt` "minLength: 1",
/// "maxLength: 10000". A longer prompt is refused here, not by Ideogram.
/// @see <https://developer.ideogram.ai/openapi/ideogram-api-v2.json> (2026-11-09)
#[test]
fn ideogram_4_5_prompt_is_capped_at_the_v2_schema_limit() {
    let reg = registry();
    let schema = reg.get("ideogram/ideogram-4-5").expect("ideogram/ideogram-4-5 missing");
    assert_eq!(schema.prompt.max_length, Some(10000), "/v2/image/generate/ideogram-4-5 prompt maxLength");
}

/// AnimateDiff's pinned version documents its seed as "Seed (0 = random,
/// maximum: 2147483647)".
/// @see <https://replicate.com/lucataco/animate-diff/versions/beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f> (2026-11-09)
#[test]
fn replicate_animatediff_seed_stops_at_the_versions_maximum() {
    let reg = registry();
    let schema = reg.get("replicate/video").expect("replicate/video missing");
    match schema.params.get("seed") {
        Some(ParamSpec::Seed(s)) => assert_eq!(s.max, 2_147_483_647, "AnimateDiff seed maximum"),
        other => panic!("replicate/video seed is {other:?}"),
    }
}

/// fal's `AuraFlowInput.guidance_scale`: "minimum": 0, "maximum": 20,
/// "default": 3.5.
/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/aura-flow> (2026-11-09)
#[test]
fn fal_auraflow_guidance_reaches_the_schema_maximum() {
    let reg = registry();
    let schema = reg.get("fal/auraflow").expect("fal/auraflow missing");
    match schema.params.get("guidance_scale") {
        Some(ParamSpec::Float(f)) => assert_eq!((f.min, f.max, f.default), (Some(0.0), Some(20.0), Some(3.5)), "AuraFlow guidance_scale"),
        other => panic!("fal/auraflow guidance_scale is {other:?}"),
    }
}

/// `stability/sd3-large` and `stability/sd3-turbo` run `sd3.5-large` and
/// `sd3.5-large-turbo` (`image/stability.rs`), which Stability's price table
/// names "Stable Diffusion 3.5 Large" and "Stable Diffusion 3.5 Large Turbo".
/// @see <https://docs.stability.ai/pricing.md> (2026-11-09)
#[test]
fn stability_sd3_rows_are_named_for_the_sd3_5_models_they_run() {
    let reg = registry();
    for (id, name) in [
        ("stability/sd3-large", "Stable Diffusion 3.5 Large"),
        ("stability/sd3-turbo", "Stable Diffusion 3.5 Large Turbo"),
    ] {
        let schema = reg.get(id).unwrap_or_else(|| panic!("{id} missing"));
        assert_eq!(schema.display_name, name, "{id} display_name");
    }
}

/// The rows whose frame image sets the clip's shape (E5): with a
/// `first_frame`, `last_frame` or `init` image their adapter sends no ratio,
/// because the vendor takes none, so the validator refuses (`strict`) or drops
/// and names `aspect_ratio` instead of the adapter leaving it out unreported.
/// `always` where a `reference` image beside the frame changes nothing;
/// `without_references` everywhere else (a reference goes to a mode that takes
/// a ratio, or the vendor refuses the mix before anything is made).
#[test]
fn every_row_whose_frames_set_the_shape_declares_it() {
    use litegen::capabilities::FramesSetShape::{self, Always, WithoutReferences};
    use std::collections::BTreeMap;
    let expected: BTreeMap<String, FramesSetShape> = [
        // Kling: `kling_settings` puts `aspect_ratio` in text-to-video only.
        ("kling/video-kling-v2-5-turbo", WithoutReferences),
        ("kling/video-kling-v2-6", WithoutReferences),
        ("kling/video-kling-v3", WithoutReferences),
        ("kling/video-kling-v3-turbo", WithoutReferences),
        ("kling/video-kling-v4", WithoutReferences),
        // TokenHub's Kling routes, through the same `kling_settings`.
        ("tokenhub/kling-video-o1", WithoutReferences),
        ("tokenhub/kling-video-v2.5-turbo", WithoutReferences),
        ("tokenhub/kling-video-v2.6", WithoutReferences),
        ("tokenhub/kling-video-v3", WithoutReferences),
        ("tokenhub/kling-video-v3-omni", WithoutReferences),
        ("tokenhub/kling-video-v3-turbo", WithoutReferences),
        ("tokenhub/kling-video-v4", WithoutReferences),
        // Vidu: only text2video and reference2video take an aspect ratio.
        ("vidu/viduq1", WithoutReferences),
        ("vidu/vidu2.0", WithoutReferences),
        ("vidu/viduq2-pro", WithoutReferences),
        ("vidu/viduq3-turbo", WithoutReferences),
        ("vidu/viduq3-pro", WithoutReferences),
        ("vidu/viduq4-preview", WithoutReferences),
        // PixVerse: only text/ takes an aspect ratio.
        ("pixverse/v3.5", WithoutReferences),
        ("pixverse/v4.5", WithoutReferences),
        ("pixverse/v5", WithoutReferences),
        ("pixverse/v6", WithoutReferences),
        ("pixverse/c1", WithoutReferences),
        ("pixverse/v6-max", WithoutReferences),
        // TokenHub: Hy, PixVerse, Vidu and MiniMax H3 take a ratio on text (and references) only.
        ("tokenhub/hy-video-v1.5", WithoutReferences),
        ("tokenhub/pixverse-video-v6.0", WithoutReferences),
        ("tokenhub/pixverse-video-c1", WithoutReferences),
        ("tokenhub/pixverse-video-v5.6", WithoutReferences),
        ("tokenhub/vidu-video-q4-preview", WithoutReferences),
        ("tokenhub/vidu-video-q3-pro", WithoutReferences),
        ("tokenhub/vidu-video-q3-turbo", WithoutReferences),
        ("tokenhub/vidu-video-q2-pro", WithoutReferences),
        ("tokenhub/minimax-video-h3", WithoutReferences),
        ("tokenhub/minimax-video-h3-max", WithoutReferences),
        // Ideogram: the three models without `i2v_aspect`.
        ("ideogram/kling-3-standard", WithoutReferences),
        ("ideogram/minimax-h3", WithoutReferences),
        ("ideogram/seedance-2-5", WithoutReferences),
        // Runway resold: image-to-video's ratio is `Shape::None` or `Shape::AutoTier`.
        // Kling 4 and 4 Flash stay on image-to-video with their references.
        ("runway/kling2.5_turbo_pro", WithoutReferences),
        ("runway/kling_4", Always),
        ("runway/kling_4_flash", Always),
        ("runway/grok_imagine_1_5", WithoutReferences),
        ("runway/grok_imagine_1_5_lite", WithoutReferences),
        ("runway/happyhorse_1_0", WithoutReferences),
        // MiniMax H3 direct: a first/last-frame request's ratio is "ignored and
        // treated as `adaptive`", and frames and references "cannot be mixed".
        ("minimax/MiniMax-H3", WithoutReferences),
        ("minimax/MiniMax-H3-Highspeed", WithoutReferences),
        ("minimax/MiniMax-H3-Lite", WithoutReferences),
        ("minimax/MiniMax-H3-Max", WithoutReferences),
    ]
    .into_iter()
    .map(|(id, shape)| (id.to_string(), shape))
    .collect();
    let declared: BTreeMap<String, FramesSetShape> = registry()
        .all()
        .filter_map(|m| match m.params.get("aspect_ratio") {
            Some(ParamSpec::AspectRatio(a)) => a.frames_set_shape.map(|shape| (m.id.clone(), shape)),
            _ => None,
        })
        .collect();
    let missing: Vec<_> = expected.iter().filter(|(id, shape)| declared.get(*id) != Some(*shape)).collect();
    let unexpected: Vec<_> = declared.iter().filter(|(id, _)| !expected.contains_key(*id)).collect();
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{} of {} rows lack (or differ from) their frames_set_shape: {missing:?}; declared but not expected: {unexpected:?}",
        missing.len(),
        expected.len()
    );
}
