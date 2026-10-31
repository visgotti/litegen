pub mod openai;
pub mod stability;
pub mod replicate;
pub mod google;
pub mod bedrock;
pub mod bfl;
pub mod bytedance;
pub mod fal;
pub mod hunyuan;
pub mod ideogram;
pub mod kling;
pub mod leonardo;
pub mod luma;
pub mod minimax;
pub mod pixellab;
pub mod recraft;
pub mod runway;
pub mod tripo;
pub mod mock;
pub mod visual_mock;

use crate::providers::ProviderError;

/// Download the image a vendor's finished job names: without credentials,
/// only from a public address (`public_only`; the tests' mock vendors run on
/// loopback), and at most 64 MiB. The vendor has run and billed the job by
/// then, so no failure here is worth a second call.
pub(crate) async fn fetch_result(
    client: &reqwest::Client,
    url: &str,
    public_only: bool,
    vendor: &str,
) -> Result<Vec<u8>, ProviderError> {
    use crate::providers::model3d::download::{fetch_asset, AssetError};
    match fetch_asset(client, url, 64 * 1024 * 1024, public_only, vendor).await {
        Ok((bytes, _)) => Ok(bytes),
        Err(AssetError::Transient(ProviderError::RequestFailed { message, status_code, provider_error, .. })) => {
            Err(ProviderError::RequestFailed { message, status_code, provider_error, retryable: false })
        }
        Err(AssetError::Transient(e)) => Err(e),
        Err(AssetError::Gone(why)) => Err(ProviderError::RequestFailed {
            message: format!("{vendor} result could not be fetched: {why}"),
            status_code: None,
            provider_error: None,
            retryable: false,
        }),
    }
}
