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
    mut response: reqwest::Response,
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
            .is_some_and(|size| size > MAX_BYTES as u64)
    {
        return Err(ProviderReadError::Invalid);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProviderReadError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err(ProviderReadError::Invalid);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ProviderReadError::Invalid)
}
