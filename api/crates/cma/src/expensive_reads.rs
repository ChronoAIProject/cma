//! Per-process admission for buffered reads, independent of control mutations.
use std::{sync::Arc, time::Duration};

use axum::{
    body::to_bytes,
    extract::{MatchedPath, Request, State},
    http::{header, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer1_public::rest::HttpProblem;
use tokio::sync::Semaphore;

mod retained;

// Match the existing native-history byte ceiling. Two concurrent responses
// bound admitted response payloads to 128 MiB per process. The optional frame
// policy keeps admission charged after body EOF while bytes remain retained.
// Store/provider working memory and total RSS require separate qualification.
const CONCURRENT_READS: usize = 2;
const RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const METADATA_BYTES: usize = 256 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) struct ReadBudget {
    retain_frames: bool,
    slots: Arc<Semaphore>,
    response_bytes: usize,
    timeout: Duration,
}

impl ReadBudget {
    pub(crate) fn new(retain_frames: bool) -> Arc<Self> {
        Arc::new(Self {
            retain_frames,
            slots: Arc::new(Semaphore::new(CONCURRENT_READS)),
            response_bytes: RESPONSE_BYTES,
            timeout: READ_TIMEOUT,
        })
    }
}

pub(crate) async fn enforce(
    State(budget): State<Arc<ReadBudget>>,
    request: Request,
    next: Next,
) -> Response {
    if !expensive(&request) {
        return next.run(request).await;
    }
    let Ok(permit) = budget.slots.clone().try_acquire_owned() else {
        return private(
            HttpProblem::RateLimited {
                retry_after_seconds: 1,
            }
            .into_response(),
        );
    };
    let metadata = request
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|path| {
            matches!(
                path.as_str(),
                "/api/v1/agents/{id}/artifacts" | "/api/v1/agents/{id}/artifacts/{artifact_id}"
            )
        });
    let byte_limit = if metadata {
        budget.response_bytes.min(METADATA_BYTES)
    } else {
        budget.response_bytes
    };
    // Refuse before running downstream rate-limit/activity/store/provider I/O.
    // Buffer fully before releasing any bytes, including chunked bodies with
    // misleading or absent Content-Length. Dropped requests release the slot.
    let result = tokio::time::timeout(budget.timeout, async {
        let response = next.run(request).await;
        let (parts, body) = response.into_parts();
        let bytes = to_bytes(body, byte_limit).await?;
        Ok::<_, axum::Error>((parts, bytes))
    })
    .await;
    let Ok(Ok((parts, bytes))) = result else {
        return private(HttpProblem::RequestBudgetUnavailable.into_response());
    };
    let body = retained::body(bytes, permit, budget.retain_frames);
    Response::from_parts(parts, body)
}

fn expensive(request: &Request) -> bool {
    if !matches!(request.method(), &Method::GET | &Method::HEAD) {
        return false;
    }
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or_else(|| request.uri().path());
    if path == "/api/v1/documents" || path.starts_with("/api/v1/documents/") {
        return true;
    }
    matches!(
        path,
        "/api/v1/agents/{id}/events"
            | "/api/v1/agents/{id}/transcript"
            | "/api/v1/agents/{id}/history"
            | "/api/v1/agents/{id}/usage"
            | "/api/v1/agents/{id}/resources"
            | "/api/v1/agents/{id}/artifacts"
            | "/api/v1/agents/{id}/artifacts/{artifact_id}"
            | "/api/v1/agents/{id}/artifacts/{artifact_id}/download"
            | "/api/v1/agents/{id}/files/tree"
            | "/api/v1/agents/{id}/files/content"
            | "/api/v1/agents/{id}/files/download"
    )
}

fn private(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
        .headers_mut()
        .append(header::LINK, crate::REST_LINKS.clone());
    response
}

#[cfg(test)]
mod tests;
