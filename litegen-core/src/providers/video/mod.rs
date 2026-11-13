pub mod bedrock;
pub mod bytedance;
pub mod google;
pub mod hunyuan;
pub mod ideogram;
pub mod tokenhub;
pub mod kling;
pub mod leonardo;
pub mod minimax;
pub mod pixverse;
pub mod runway;
pub mod luma;
pub mod vidu;
pub mod mock;
pub mod openai;
pub mod fal;
pub mod replicate;
pub mod visual_mock;

use crate::providers::ProviderError;

/// Read a video poll response body as JSON, turning transport / HTTP-status /
/// parse failures into a `ProviderError` instead of silently yielding
/// `Value::Null`.
///
/// Every provider's poll_status maps an unrecognised body to `Pending`/
/// `Processing`, so without this an upstream `401`/`404`/`5xx` (or a non-JSON
/// error page) would be reported as "still processing" forever — the generation
/// never terminalises and the poller keeps re-polling a dead job. A terminal
/// `4xx` (auth revoked, job purged) is reported non-retryably so the poller can
/// fail the row; `5xx`/`408`/`429` are transient and retryable.
pub(crate) async fn read_poll_json(
    resp: reqwest::Response,
    provider: &str,
) -> Result<serde_json::Value, ProviderError> {
    let status = resp.status();
    if !status.is_success() {
        let retryable = status.is_server_error()
            || status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        return Err(ProviderError::RequestFailed {
            message: format!("{provider} poll returned HTTP {status}: {snippet}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable,
        });
    }
    resp.json().await.map_err(|e| ProviderError::RequestFailed {
        message: format!("{provider} poll returned an unparseable body: {e}"),
        status_code: Some(status.as_u16()),
        provider_error: None,
        retryable: false,
    })
}

/// A poll answer naming a job state its vendor doesn't document (or none):
/// the job fails now, naming it, through the normal terminal path (its
/// webhook fires), rather than polling until the reaper's maximum age.
/// Every adapter lists its vendor's in-progress states and sends anything
/// else here, as Replicate's and BFL's adapters did first (2026-10-30 audit).
/// What a poll answers when its gate said another observer keeps the
/// finished video ([`crate::providers::Fetch::StandAside`]): still
/// processing, short of done, nothing downloaded and no error.
pub(crate) fn standing_aside() -> crate::providers::VideoGenerationPollResult {
    crate::providers::VideoGenerationPollResult {
        status: crate::types::GenerationStatus::Processing,
        progress: 99,
        video_url: None,
        video_data: None,
        content_type: None,
        error: None,
        metadata: Default::default(),
    }
}

pub(crate) fn unknown_state(vendor: &str, state: Option<&str>) -> crate::providers::VideoGenerationPollResult {
    crate::providers::VideoGenerationPollResult {
        status: crate::types::GenerationStatus::Failed,
        progress: 0,
        video_url: None,
        video_data: None,
        content_type: None,
        error: Some(unknown_state_message(vendor, state)),
        metadata: Default::default(),
    }
}

/// `{vendor} reported job status '{state}'`, the message every family uses.
pub(crate) fn unknown_state_message(vendor: &str, state: Option<&str>) -> String {
    match state {
        Some(s) => format!("{vendor} reported job status '{s}', which litegen does not know"),
        None => format!("{vendor} answered without a job status"),
    }
}

#[cfg(test)]
mod poll_json_tests {
    use super::read_poll_json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn get(uri: &str) -> reqwest::Response {
        reqwest::Client::new().get(uri).send().await.unwrap()
    }

    #[tokio::test]
    async fn errors_non_retryably_on_4xx() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401).set_body_string("{\"detail\":\"invalid api key\"}"))
            .mount(&s)
            .await;
        match read_poll_json(get(&s.uri()).await, "luma").await.unwrap_err() {
            crate::providers::ProviderError::RequestFailed { retryable, status_code, .. } => {
                assert!(!retryable, "a 4xx poll must be terminal, not re-polled forever");
                assert_eq!(status_code, Some(401));
            }
            other => panic!("expected RequestFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn errors_retryably_on_5xx() {
        let s = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&s).await;
        match read_poll_json(get(&s.uri()).await, "luma").await.unwrap_err() {
            crate::providers::ProviderError::RequestFailed { retryable, .. } => {
                assert!(retryable, "a 5xx poll is transient and should be retried");
            }
            other => panic!("expected RequestFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn parses_successful_body() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"state":"completed"})))
            .mount(&s)
            .await;
        let v = read_poll_json(get(&s.uri()).await, "luma").await.unwrap();
        assert_eq!(v["state"], "completed");
    }

    #[tokio::test]
    async fn errors_on_unparseable_success_body() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
            .mount(&s)
            .await;
        assert!(
            read_poll_json(get(&s.uri()).await, "luma").await.is_err(),
            "a 200 with a non-JSON body must not be treated as an empty (pending) result"
        );
    }
}
