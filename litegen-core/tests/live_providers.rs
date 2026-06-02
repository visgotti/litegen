//! Live provider integration tests.
//!
//! These hit REAL provider APIs and cost real money, so every test is
//! `#[ignore]`d AND self-skips (early return) when its credential env var(s)
//! are absent. Normal `cargo test` never runs them. To run the ones you have
//! keys for:
//!
//!     cargo test -p litegen --test live_providers -- --ignored --nocapture
//!
//! Put credentials in `litegen-core/.env` (see `.env.example`); they're loaded
//! automatically below. A test with missing keys prints "skip:" and returns Ok.
//!
//! Each test exercises the real generate (and, for video, poll-to-completion)
//! path and asserts a non-empty image or a result video URL.

use std::path::PathBuf;
use std::time::Duration;

use litegen::capabilities::CapabilityRegistry;
use litegen::proxy::materializer::{Cleanup, MaterializedRequest};
use litegen::providers::{
    ImageExtras, ImageProvider, ProviderInstanceConfig, VideoExtras, VideoGenerationHandle,
    VideoProvider,
};
use litegen::types::{BaseGenerationRequest, GenerationStatus};

// ─── Helpers ────────────────────────────────────────────────────────────────

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// Load .env once (best-effort) so credentials are available.
fn load_env() {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(".env");
    let _ = dotenvy::from_path(&p);
}

fn schema(id: &str) -> litegen::capabilities::ModelSchema {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("models");
    CapabilityRegistry::from_dir(&p)
        .expect("load models dir")
        .get(id)
        .unwrap_or_else(|| panic!("model {id} not found in registry"))
        .clone()
}

fn base(prompt: &str, model: &str) -> BaseGenerationRequest {
    BaseGenerationRequest {
        prompt: prompt.to_string(),
        model: model.to_string(),
        n: 1,
        negative_prompt: None,
        seed: None,
        reference_images: vec![],
        strict: true,
        extra: None,
        metadata: None,
    }
}

fn img_extras() -> ImageExtras {
    ImageExtras {
        size: None,
        aspect_ratio: Some("1:1".to_string()),
        quality: None,
        style: None,
        steps: None,
        guidance_scale: None,
        strength: None,
        response_format: "url".to_string(),
        extra: None,
    }
}

fn vid_extras() -> VideoExtras {
    VideoExtras {
        duration_seconds: 5.0,
        aspect_ratio: Some("16:9".to_string()),
        resolution: None,
        fps: None,
        extra: None,
    }
}

fn empty_mat() -> MaterializedRequest {
    MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
}

fn api_key_cfg(key: &str) -> ProviderInstanceConfig {
    let mut cfg = ProviderInstanceConfig::default();
    cfg.credentials.api_key = Some(key.to_string());
    cfg
}

fn keypair_cfg(key_id: &str, key_secret: &str, region: Option<&str>) -> ProviderInstanceConfig {
    let mut cfg = ProviderInstanceConfig::default();
    cfg.credentials.key_id = Some(key_id.to_string());
    cfg.credentials.key_secret = Some(key_secret.to_string());
    cfg.credentials.region = region.map(|s| s.to_string());
    cfg
}

/// Poll a submitted video job to a terminal state (≤ ~6 min) and return the URL.
async fn drive_video(provider: &dyn VideoProvider, handle: VideoGenerationHandle) -> Option<String> {
    for _ in 0..72 {
        tokio::time::sleep(Duration::from_secs(5)).await;
        match provider.poll_status(&handle).await {
            Ok(p) if p.status == GenerationStatus::Completed => return p.video_url,
            Ok(p) if p.status == GenerationStatus::Failed => {
                panic!("video generation failed: {:?}", p.error)
            }
            Ok(_) => continue,
            Err(e) => panic!("poll_status error: {e}"),
        }
    }
    panic!("video generation did not complete within timeout")
}

macro_rules! skip_if_missing {
    ($($var:expr),+) => {{
        load_env();
        let mut missing = vec![];
        $( if env($var).is_none() { missing.push($var); } )+
        if !missing.is_empty() {
            eprintln!("skip: missing env {:?}", missing);
            return;
        }
    }};
}

// ─── Image providers ──────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: set GOOGLE_API_KEY"]
async fn live_google_image() {
    skip_if_missing!("GOOGLE_API_KEY");
    let mut p = litegen::providers::image::google::GoogleProvider::new();
    p.configure(api_key_cfg(&env("GOOGLE_API_KEY").unwrap()));
    let out = p
        .generate(&schema("google/gemini-2.5-flash-image"), &base("a red apple on a table", "google/gemini-2.5-flash-image"), &img_extras(), &empty_mat())
        .await
        .expect("google image");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set LUMA_API_KEY"]
async fn live_luma_photon_image() {
    skip_if_missing!("LUMA_API_KEY");
    let mut p = litegen::providers::image::luma::LumaImageProvider::new();
    p.configure(api_key_cfg(&env("LUMA_API_KEY").unwrap()));
    let out = p.generate(&schema("luma/photon-1"), &base("a serene lake", "luma/photon-1"), &img_extras(), &empty_mat()).await.expect("luma photon");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set RUNWAY_API_KEY"]
async fn live_runway_image() {
    skip_if_missing!("RUNWAY_API_KEY");
    let mut p = litegen::providers::image::runway::RunwayImageProvider::new();
    p.configure(api_key_cfg(&env("RUNWAY_API_KEY").unwrap()));
    let out = p.generate(&schema("runway/gen4_image"), &base("a neon city", "runway/gen4_image"), &img_extras(), &empty_mat()).await.expect("runway image");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set BFL_API_KEY"]
async fn live_bfl_image() {
    skip_if_missing!("BFL_API_KEY");
    let mut p = litegen::providers::image::bfl::BflProvider::new();
    p.configure(api_key_cfg(&env("BFL_API_KEY").unwrap()));
    let out = p.generate(&schema("bfl/flux-pro-1.1"), &base("a cyberpunk fox", "bfl/flux-pro-1.1"), &img_extras(), &empty_mat()).await.expect("bfl");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set IDEOGRAM_API_KEY"]
async fn live_ideogram_image() {
    skip_if_missing!("IDEOGRAM_API_KEY");
    let mut p = litegen::providers::image::ideogram::IdeogramProvider::new();
    p.configure(api_key_cfg(&env("IDEOGRAM_API_KEY").unwrap()));
    let out = p.generate(&schema("ideogram/ideogram-v3"), &base("a vintage poster", "ideogram/ideogram-v3"), &img_extras(), &empty_mat()).await.expect("ideogram");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set RECRAFT_API_TOKEN"]
async fn live_recraft_image() {
    skip_if_missing!("RECRAFT_API_TOKEN");
    let mut p = litegen::providers::image::recraft::RecraftProvider::new();
    p.configure(api_key_cfg(&env("RECRAFT_API_TOKEN").unwrap()));
    let out = p.generate(&schema("recraft/recraftv3"), &base("a robot mascot", "recraft/recraftv3"), &img_extras(), &empty_mat()).await.expect("recraft");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set MINIMAX_API_KEY"]
async fn live_minimax_image() {
    skip_if_missing!("MINIMAX_API_KEY");
    let mut p = litegen::providers::image::minimax::MiniMaxImageProvider::new();
    p.configure(api_key_cfg(&env("MINIMAX_API_KEY").unwrap()));
    let out = p.generate(&schema("minimax/image-01"), &base("a koi pond", "minimax/image-01"), &img_extras(), &empty_mat()).await.expect("minimax");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set BYTEDANCE_API_KEY"]
async fn live_bytedance_seedream_image() {
    skip_if_missing!("BYTEDANCE_API_KEY");
    let mut p = litegen::providers::image::bytedance::ByteDanceImageProvider::new();
    p.configure(api_key_cfg(&env("BYTEDANCE_API_KEY").unwrap()));
    let out = p.generate(&schema("bytedance/seedream-4-0-250828"), &base("an ornate map", "bytedance/seedream-4-0-250828"), &img_extras(), &empty_mat()).await.expect("seedream");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set LEONARDO_API_KEY"]
async fn live_leonardo_image() {
    skip_if_missing!("LEONARDO_API_KEY");
    let mut p = litegen::providers::image::leonardo::LeonardoImageProvider::new();
    p.configure(api_key_cfg(&env("LEONARDO_API_KEY").unwrap()));
    let out = p.generate(&schema("leonardo/diffusion-xl"), &base("an enchanted forest", "leonardo/diffusion-xl"), &img_extras(), &empty_mat()).await.expect("leonardo");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set KLING_ACCESS_KEY + KLING_SECRET_KEY"]
async fn live_kling_image() {
    skip_if_missing!("KLING_ACCESS_KEY", "KLING_SECRET_KEY");
    let mut p = litegen::providers::image::kling::KlingImageProvider::new();
    p.configure(keypair_cfg(&env("KLING_ACCESS_KEY").unwrap(), &env("KLING_SECRET_KEY").unwrap(), None));
    let out = p.generate(&schema("kling/kling-v2"), &base("a jade dragon", "kling/kling-v2"), &img_extras(), &empty_mat()).await.expect("kling image");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set BEDROCK_ACCESS_KEY_ID + BEDROCK_SECRET_ACCESS_KEY (+BEDROCK_REGION)"]
async fn live_bedrock_canvas_image() {
    skip_if_missing!("BEDROCK_ACCESS_KEY_ID", "BEDROCK_SECRET_ACCESS_KEY");
    let region = env("BEDROCK_REGION").unwrap_or_else(|| "us-east-1".to_string());
    let mut p = litegen::providers::image::bedrock::BedrockImageProvider::new();
    p.configure(keypair_cfg(&env("BEDROCK_ACCESS_KEY_ID").unwrap(), &env("BEDROCK_SECRET_ACCESS_KEY").unwrap(), Some(&region)));
    let out = p.generate(&schema("bedrock/amazon.nova-canvas-v1:0"), &base("a desert at golden hour", "bedrock/amazon.nova-canvas-v1:0"), &img_extras(), &empty_mat()).await.expect("bedrock canvas");
    assert!(!out.data.is_empty());
}

#[tokio::test]
#[ignore = "live: set TENCENT_SECRET_ID + TENCENT_SECRET_KEY"]
async fn live_hunyuan_image() {
    skip_if_missing!("TENCENT_SECRET_ID", "TENCENT_SECRET_KEY");
    let region = env("TENCENT_REGION").unwrap_or_else(|| "ap-guangzhou".to_string());
    let mut p = litegen::providers::image::hunyuan::HunyuanImageProvider::new();
    p.configure(keypair_cfg(&env("TENCENT_SECRET_ID").unwrap(), &env("TENCENT_SECRET_KEY").unwrap(), Some(&region)));
    let out = p.generate(&schema("hunyuan/hunyuan-image"), &base("a tranquil zen garden", "hunyuan/hunyuan-image"), &img_extras(), &empty_mat()).await.expect("hunyuan image");
    assert!(!out.data.is_empty());
}

// ─── Video providers ──────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: set GOOGLE_API_KEY"]
async fn live_google_veo_video() {
    skip_if_missing!("GOOGLE_API_KEY");
    let mut p = litegen::providers::video::google::GoogleVideoProvider::new();
    p.configure(api_key_cfg(&env("GOOGLE_API_KEY").unwrap()));
    let h = p.generate(&schema("google/veo-3.0-generate-001"), &base("a cat surfing a wave", "google/veo-3.0-generate-001"), &vid_extras(), &empty_mat()).await.expect("veo submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set LUMA_API_KEY"]
async fn live_luma_video() {
    skip_if_missing!("LUMA_API_KEY");
    let mut p = litegen::providers::video::luma::LumaProvider::new();
    p.configure(api_key_cfg(&env("LUMA_API_KEY").unwrap()));
    let h = p.generate(&schema("luma/ray-2"), &base("a sunset over the ocean", "luma/ray-2"), &vid_extras(), &empty_mat()).await.expect("luma submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set MINIMAX_API_KEY"]
async fn live_minimax_video() {
    skip_if_missing!("MINIMAX_API_KEY");
    let mut p = litegen::providers::video::minimax::MiniMaxVideoProvider::new();
    p.configure(api_key_cfg(&env("MINIMAX_API_KEY").unwrap()));
    let h = p.generate(&schema("minimax/MiniMax-Hailuo-02"), &base("a drone shot over a canyon", "minimax/MiniMax-Hailuo-02"), &vid_extras(), &empty_mat()).await.expect("minimax submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set VIDU_API_KEY"]
async fn live_vidu_video() {
    skip_if_missing!("VIDU_API_KEY");
    let mut p = litegen::providers::video::vidu::ViduProvider::new();
    p.configure(api_key_cfg(&env("VIDU_API_KEY").unwrap()));
    let h = p.generate(&schema("vidu/viduq1"), &base("a hummingbird in slow motion", "vidu/viduq1"), &vid_extras(), &empty_mat()).await.expect("vidu submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set PIXVERSE_API_KEY"]
async fn live_pixverse_video() {
    skip_if_missing!("PIXVERSE_API_KEY");
    let mut p = litegen::providers::video::pixverse::PixverseProvider::new();
    p.configure(api_key_cfg(&env("PIXVERSE_API_KEY").unwrap()));
    let h = p.generate(&schema("pixverse/v4.5"), &base("a paper airplane gliding", "pixverse/v4.5"), &vid_extras(), &empty_mat()).await.expect("pixverse submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set BYTEDANCE_API_KEY"]
async fn live_bytedance_seedance_video() {
    skip_if_missing!("BYTEDANCE_API_KEY");
    let mut p = litegen::providers::video::bytedance::ByteDanceVideoProvider::new();
    p.configure(api_key_cfg(&env("BYTEDANCE_API_KEY").unwrap()));
    let h = p.generate(&schema("bytedance/doubao-seedance-1-0-pro-250528"), &base("a timelapse of city traffic", "bytedance/doubao-seedance-1-0-pro-250528"), &vid_extras(), &empty_mat()).await.expect("seedance submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set KLING_ACCESS_KEY + KLING_SECRET_KEY"]
async fn live_kling_video() {
    skip_if_missing!("KLING_ACCESS_KEY", "KLING_SECRET_KEY");
    let mut p = litegen::providers::video::kling::KlingVideoProvider::new();
    p.configure(keypair_cfg(&env("KLING_ACCESS_KEY").unwrap(), &env("KLING_SECRET_KEY").unwrap(), None));
    let h = p.generate(&schema("kling/video-kling-v2-1"), &base("a phoenix rising", "kling/video-kling-v2-1"), &vid_extras(), &empty_mat()).await.expect("kling submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set TENCENT_SECRET_ID + TENCENT_SECRET_KEY"]
async fn live_hunyuan_video() {
    skip_if_missing!("TENCENT_SECRET_ID", "TENCENT_SECRET_KEY");
    let region = env("TENCENT_REGION").unwrap_or_else(|| "ap-guangzhou".to_string());
    let mut p = litegen::providers::video::hunyuan::HunyuanVideoProvider::new();
    p.configure(keypair_cfg(&env("TENCENT_SECRET_ID").unwrap(), &env("TENCENT_SECRET_KEY").unwrap(), Some(&region)));
    let h = p.generate(&schema("hunyuan/hunyuan-video"), &base("a calligraphy brush coming alive", "hunyuan/hunyuan-video"), &vid_extras(), &empty_mat()).await.expect("hunyuan submit");
    assert!(drive_video(&p, h).await.is_some());
}

#[tokio::test]
#[ignore = "live: set BEDROCK_* and BEDROCK_S3_OUTPUT_URI"]
async fn live_bedrock_reel_video() {
    skip_if_missing!("BEDROCK_ACCESS_KEY_ID", "BEDROCK_SECRET_ACCESS_KEY", "BEDROCK_S3_OUTPUT_URI");
    let region = env("BEDROCK_REGION").unwrap_or_else(|| "us-east-1".to_string());
    let mut cfg = keypair_cfg(&env("BEDROCK_ACCESS_KEY_ID").unwrap(), &env("BEDROCK_SECRET_ACCESS_KEY").unwrap(), Some(&region));
    cfg.options = Some(serde_json::json!({ "s3_output_uri": env("BEDROCK_S3_OUTPUT_URI").unwrap() }));
    let mut p = litegen::providers::video::bedrock::BedrockVideoProvider::new();
    p.configure(cfg);
    let h = p.generate(&schema("bedrock/amazon.nova-reel-v1:1"), &base("waves crashing on rocks", "bedrock/amazon.nova-reel-v1:1"), &vid_extras(), &empty_mat()).await.expect("reel submit");
    assert!(drive_video(&p, h).await.is_some());
}
