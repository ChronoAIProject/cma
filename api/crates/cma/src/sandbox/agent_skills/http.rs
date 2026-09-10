use super::*;

pub(super) async fn bounded_body(
    response: reqwest::Response,
    index: usize,
) -> Result<Vec<u8>, AgentSkillResolutionError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(invalid(index, "Ornn response exceeds the byte limit"));
    }
    let mut body = Vec::with_capacity(
        response
            .content_length()
            .and_then(|length| usize::try_from(length).ok())
            .unwrap_or_default()
            .min(MAX_RESPONSE_BYTES),
    );
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| unavailable_at(index, "Ornn response read failed"))?;
        if body
            .len()
            .checked_add(chunk.len())
            .is_none_or(|length| length > MAX_RESPONSE_BYTES)
        {
            return Err(invalid(index, "Ornn response exceeds the byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(super) fn classify_status(
    index: usize,
    status: reqwest::StatusCode,
) -> Result<(), AgentSkillResolutionError> {
    if status.is_success() {
        return Ok(());
    }
    match status {
        reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::NOT_FOUND => {
            Err(AgentSkillResolutionError::NotFound(format!(
                "skill reference {index} is absent or inaccessible"
            )))
        }
        reqwest::StatusCode::UNAUTHORIZED
        | reqwest::StatusCode::REQUEST_TIMEOUT
        | reqwest::StatusCode::TOO_MANY_REQUESTS
        | reqwest::StatusCode::BAD_GATEWAY
        | reqwest::StatusCode::SERVICE_UNAVAILABLE
        | reqwest::StatusCode::GATEWAY_TIMEOUT => {
            Err(unavailable_at(index, "Ornn is temporarily unavailable"))
        }
        status if status.is_server_error() => {
            Err(unavailable_at(index, "Ornn is temporarily unavailable"))
        }
        _ => Err(invalid(index, "Ornn refused the exact pinned reference")),
    }
}
