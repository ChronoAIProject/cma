//! Bounded identity-provider metadata transport shared by resource adapters.
use serde::de::DeserializeOwned;
use zeroize::Zeroizing;

const MAX_BYTES: usize = 1_048_576;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderReadError {
    Unavailable,
    Invalid,
    Refused(u16),
}

/// Content-free observations; callers retain their existing public error policy.
pub enum EvidenceReadError {
    Timeout,
    Read,
    Partial,
    TooLarge,
    InvalidJson,
    Refused(u16),
}

pub async fn read_json<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ProviderReadError> {
    read_json_limited(response, MAX_BYTES).await
}

pub async fn read_json_limited<T: DeserializeOwned>(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<T, ProviderReadError> {
    read_evidence(response, max_bytes)
        .await
        .map_err(|error| match error {
            EvidenceReadError::Timeout | EvidenceReadError::Read => ProviderReadError::Unavailable,
            EvidenceReadError::Refused(status) => ProviderReadError::Refused(status),
            _ => ProviderReadError::Invalid,
        })
}

pub async fn read_evidence<T: DeserializeOwned>(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<T, EvidenceReadError> {
    if !response.status().is_success() {
        return Err(EvidenceReadError::Refused(response.status().as_u16()));
    }
    if response.headers().contains_key(reqwest::header::LINK)
        || response
            .headers()
            .contains_key(reqwest::header::CONTENT_RANGE)
    {
        return Err(EvidenceReadError::Partial);
    }
    if response
        .content_length()
        .is_some_and(|size| size > max_bytes as u64)
    {
        return Err(EvidenceReadError::TooLarge);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            EvidenceReadError::Timeout
        } else {
            EvidenceReadError::Read
        }
    })? {
        if bytes.len().saturating_add(chunk.len()) > max_bytes {
            return Err(EvidenceReadError::TooLarge);
        }
        // Do not let Vec's geometric growth exceed the transport byte budget.
        bytes.reserve_exact(chunk.len());
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| EvidenceReadError::InvalidJson)
}
