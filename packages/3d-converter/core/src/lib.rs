//! litegen's 3D converter as a library: the typed, high-level API that the npm
//! and PyPI bindings wrap. Everything it converts with is `litegen-mesh`, the
//! same source litegen's HTTP converter runs.

mod detect;
mod error;
mod format;
mod info;
mod input;
mod loss;
mod model;
mod options;

pub use detect::detect_format;
pub use error::{Error, ErrorCode};
pub use format::{formats, Format, FormatInfo, Keeps, MaterialLayer, MaterialSupport, StoredFrame};
pub use info::{AnimationInfo, BindPose, Bounds, Humanoid, HumanoidBone, HumanoidNaming, MaterialInfo, ModelInfo, SkinInfo, TextureMap, VrmBone};
pub use input::{check_input_size, FileRole, Input, InputFile, InputFileInfo};
pub use loss::{Loss, LossKind};
pub use model::{convert, Conversion, Model, OutputFile};
pub use options::{
    DaeMorphs, FbxColorSpace, LightUnits, OptionName, Packaging, ReadOptions, Unit, UpAxis, UsdClips, UsdzRoot, WriteOptions,
};

/// Largest input the converter reads, in bytes: mesh's own cap.
pub const MAX_INPUT_BYTES: usize = litegen_mesh::MAX_INPUT_BYTES;
