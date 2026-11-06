//! Bounded identity-provider metadata transport shared by resource adapters.
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

const MAX_BYTES: usize = 1_048_576;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderReadError {
    Unavailable,
    Invalid,
    Refused(u16),
}

pub(crate) async fn read_json<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ProviderReadError> {
    read_json_limited(response, MAX_BYTES).await
}

pub(super) async fn read_json_limited<T: DeserializeOwned>(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<T, ProviderReadError> {
    if !response.status().is_success() {
        return Err(ProviderReadError::Refused(response.status().as_u16()));
    }
    if response.headers().contains_key(reqwest::header::LINK)
        || response
            .headers()
            .contains_key(reqwest::header::CONTENT_RANGE)
        || response
            .content_length()
            .is_some_and(|size| size > max_bytes as u64)
    {
        return Err(ProviderReadError::Invalid);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProviderReadError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(ProviderReadError::Invalid);
        }
        // Do not let Vec's geometric growth exceed the transport byte budget.
        bytes.reserve_exact(chunk.len());
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ProviderReadError::Invalid)
}
