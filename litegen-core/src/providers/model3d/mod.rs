//! 3D model (mesh) generation providers.
//!
//! Every provider here is async submit + poll, and returns downloaded bytes so
//! the caller can re-host them on litegen storage (see `Model3dProvider`).

pub mod cube;
pub mod fal;
pub mod glb;
pub mod mock;
