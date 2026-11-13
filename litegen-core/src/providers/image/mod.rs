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
pub mod tokenhub;
pub mod tripo;
pub mod mock;
pub mod visual_mock;

use crate::providers::ProviderError;

/// A poll answer naming a job state its vendor doesn't document (or none):
/// the call fails now, naming it, and is not sent again (the vendor took the
/// job), as BFL's and Replicate's adapters did first.
pub(crate) fn unknown_state(vendor: &str, state: Option<&str>) -> ProviderError {
    ProviderError::RequestFailed {
        message: crate::providers::video::unknown_state_message(vendor, state),
        status_code: None,
        provider_error: None,
        retryable: false,
    }
}

/// One look at a job an adapter's own poll loop would wait on: the loop,
/// run under a deadline that ends after its first poll. `Ok(None)` while the
/// job is still running, or after a poll that failed in passing.
pub(crate) async fn poll_once<T>(
    poll: impl std::future::Future<Output = Result<T, ProviderError>>,
) -> Result<Option<T>, ProviderError> {
    match crate::providers::with_deadline(std::time::Instant::now() + std::time::Duration::from_millis(1), poll).await {
        Ok(v) => Ok(Some(v)),
        Err(ProviderError::Timeout { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A poll loop's final error as a job verdict: a non-retryable one is the
/// vendor's (the job failed) or a state litegen can never learn.
pub(crate) fn job_failed(e: ProviderError) -> Result<crate::providers::ImageJobPoll, ProviderError> {
    match e {
        ProviderError::RequestFailed { retryable: false, message, .. } => Ok(crate::providers::ImageJobPoll::Failed(message)),
        e => Err(e),
    }
}

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
