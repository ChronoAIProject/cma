use futures_util::StreamExt as _;
use layer2_core::profiles::authoring::AuthoringSourceError;
use serde::de::DeserializeOwned;

pub(super) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

pub(super) fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("CMA-profile-authoring/1.0")
        .build()
        .map_err(|_| "profile authoring HTTP client is not usable".into())
}

pub(super) async fn json<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, AuthoringSourceError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(invalid("response exceeds the byte limit"));
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| unavailable("response read failed"))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(invalid("response exceeds the byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| invalid("response is not valid JSON"))
}

pub(super) fn unavailable(detail: &str) -> AuthoringSourceError {
    AuthoringSourceError::Unavailable(detail.into())
}

pub(super) fn invalid(detail: &str) -> AuthoringSourceError {
    AuthoringSourceError::InvalidResponse(detail.into())
}
