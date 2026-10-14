//! Live 3D provider tests — they hit fal.ai for real and are BILLED.
//!
//! Every test is `#[ignore]`d AND self-skips when `FAL_KEY` is absent, so a
//! plain `cargo test` never runs one. To run them (TRELLIS is the cheapest 3D
//! endpoint in the catalog, $0.02 a generation on 2026-10-12):
//!
//!     cargo test -p litegen --test live_model3d -- --ignored --nocapture
//!
//! Put the key in `litegen-core/.env` (see `.env.example`).
//!
//! What only a live run proves, and the wiremock suite in
//! `providers/model3d/fal.rs` cannot: that fal's real queue still answers in
//! the shapes those tests mock, that its CDN URLs pass the public-address
//! guard, and that the GLB it returns is one litegen's converter can read.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use litegen::capabilities::CapabilityRegistry;
use litegen::providers::model3d::fal::FalModel3dProvider;
use litegen::providers::{Model3dExtras, Model3dProvider, ProviderInstanceConfig};
use litegen::proxy::materializer::{Cleanup, MaterializedRef, MaterializedRefForm, MaterializedRequest};
use litegen::proxy::model3d_convert::synthesize_missing_formats;
use litegen::types::{BaseGenerationRequest, GenerationStatus, Model3dAssetKind};

/// fal's own example input for TRELLIS, from `TrellisInput.image_url.examples`
/// in <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/trellis>.
const TRELLIS_EXAMPLE_IMAGE: &str =
    "https://storage.googleapis.com/falserverless/web-examples/rodin3d/warriorwoman.png";

fn fal_key() -> Option<String> {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(".env");
    let _ = dotenvy::from_path(&p);
    std::env::var("FAL_KEY").ok().filter(|k| !k.is_empty())
}

fn schema(id: &str) -> litegen::capabilities::ModelSchema {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("models");
    CapabilityRegistry::from_dir(&p).expect("load models").get(id).expect("model").clone()
}

#[tokio::test]
#[ignore = "live and billed ($0.02): set FAL_KEY"]
async fn live_fal_trellis_delivers_four_containers_from_one_glb() {
    let Some(key) = fal_key() else {
        eprintln!("skip: FAL_KEY not set");
        return;
    };
    let mut provider = FalModel3dProvider::new();
    provider.configure(ProviderInstanceConfig { api_key: key, ..Default::default() });

    let model = schema("fal/trellis");
    let base = BaseGenerationRequest {
        prompt: String::new(),
        model: model.id.clone(),
        n: 1,
        negative_prompt: None,
        seed: Some(42),
        reference_images: vec![],
        strict: true,
        extra: None,
        metadata: None,
    };
    let requested: Vec<String> = ["glb", "obj", "stl", "ply"].map(String::from).to_vec();
    let extras = Model3dExtras {
        output_formats: requested.clone(),
        texture: None, pbr: None, target_polycount: None,
        symmetry: None, topology: None, rig: None, extra: None,
    };
    let refs = MaterializedRequest {
        refs: vec![MaterializedRef {
            role: "init".into(),
            form: MaterializedRefForm::Url(TRELLIS_EXAMPLE_IMAGE.into()),
        }],
        cleanup: Cleanup::empty(),
    };

    let handle = provider.generate(&model, &base, &extras, &refs).await.expect("fal accepted the submit");
    eprintln!("submitted fal request {}", handle.provider_job_id);

    let deadline = Instant::now() + Duration::from_secs(600);
    let done = loop {
        match provider.poll_status(&handle).await {
            Ok(p) if p.status.is_terminal() => break p,
            Ok(p) => eprintln!("… {:?} ({}%)", p.status, p.progress),
            Err(e) if e.is_retryable() => eprintln!("… transient: {e}"),
            Err(e) => panic!("poll failed for good: {e}"),
        }
        assert!(Instant::now() < deadline, "TRELLIS did not finish within 10 minutes");
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    assert_eq!(done.status, GenerationStatus::Completed, "{:?}", done.error);

    let mesh = done.files.iter().find(|f| f.kind == Model3dAssetKind::Mesh).expect("a mesh");
    assert_eq!(mesh.format, "glb");
    assert_eq!(&mesh.bytes[0..4], b"glTF", "a real binary glTF");

    // TRELLIS emits glb only; the rest must come out of litegen's converter.
    let (files, report) = synthesize_missing_formats(done.files.clone(), &requested);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    for f in &requested {
        assert!(
            files.iter().any(|x| x.kind == Model3dAssetKind::Mesh && &x.format == f),
            "{f} was not delivered"
        );
    }
}
