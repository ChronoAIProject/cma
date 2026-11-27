use futures_util::StreamExt as _;

use super::{KeyIssuerError, ServiceListing, ServiceRow};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 1000;

pub(super) async fn rows(response: reqwest::Response) -> Result<Vec<ServiceRow>, KeyIssuerError> {
    let unavailable = |message: &str| KeyIssuerError::Unavailable(message.into());
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err(unavailable("service catalogue exceeds the byte limit"));
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| unavailable("service catalogue read failed"))?;
        if body.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err(unavailable("service catalogue exceeds the byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    let rows = serde_json::from_slice::<ServiceListing>(&body)
        .map_err(|_| unavailable("service catalogue is not valid JSON"))?
        .into_rows();
    if rows.len() > MAX_ROWS {
        return Err(unavailable("service catalogue exceeds the row limit"));
    }
    Ok(rows)
}
