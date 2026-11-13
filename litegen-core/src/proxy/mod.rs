pub mod registry;
pub mod router;
pub mod cache;
pub mod storage;
pub mod video_probe;
pub mod segmentation;
pub mod preserve;
pub mod materializer;
pub mod model3d_convert;
pub mod model3d_stages;
pub mod model3d_source;
pub mod image_edit;
pub mod model3d_deliver;
pub mod delivery_lease;
pub mod ref_publish;
pub mod webhook;
pub mod poller;
pub mod retention;
pub mod aws_sts;
pub mod circuit_breaker;
#[cfg(test)] mod materializer_tests;
// Every adapter honors the operator's provider config: a test across the
// catalog, the registry and the validator, so the server's (engine boundary D8).
#[cfg(test)] mod config_conformance;

pub use registry::ProviderRegistry;
pub use router::ProxyRouter;
pub use cache::GenerationCache;
pub use storage::{build_image_store, ImageStore, ImageStorage};
pub use poller::spawn_poller;
