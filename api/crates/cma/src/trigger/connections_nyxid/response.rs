use futures_util::StreamExt as _;
use reqwest::Response;
use serde::de::DeserializeOwned;

use super::{malformed, unavailable, ServiceConnectionError, PROVIDER_UNAVAILABLE};

const PROVIDER_RESPONSE_MAX_BYTES: usize = 1024 * 1024;

pub(super) async fn decode<T: DeserializeOwned>(
    response: Response,
) -> Result<T, ServiceConnectionError> {
    if response
        .content_length()
        .is_some_and(|length| length > PROVIDER_RESPONSE_MAX_BYTES as u64)
    {
        return Err(malformed());
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(PROVIDER_RESPONSE_MAX_BYTES),
    );
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| unavailable(PROVIDER_UNAVAILABLE))?;
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > PROVIDER_RESPONSE_MAX_BYTES)
        {
            return Err(malformed());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| malformed())
}
