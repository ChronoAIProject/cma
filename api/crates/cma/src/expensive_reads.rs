//! Per-process admission for buffered reads, independent of control mutations.
use std::{sync::Arc, time::Duration};

use axum::{
    body::to_bytes,
    extract::{MatchedPath, Request, State},
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
};
use layer1_public::rest::HttpProblem;
use tokio::sync::Semaphore;

mod lanes;
mod retained;
use lanes::Lane;

// Match the existing native-history byte ceiling. Two concurrent responses
// bound bulk payloads to 128 MiB per process. Independent two-slot fixed-shape
// Agent observations (64 KiB) and report descriptors (1 MiB) add 2.125 MiB.
// With N replicas each lane admits at most 2N reads; capacity is not pooled.
// The optional frame
// policy keeps admission charged after body EOF while bytes remain retained.
// Store/provider working memory and total RSS require separate qualification.
const CONCURRENT_READS: usize = 2;
const RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const METADATA_BYTES: usize = 256 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) struct ReadBudget {
    retain_frames: bool,
    slots: Arc<Semaphore>,
    observations: Arc<Semaphore>,
    reports: Arc<Semaphore>,
    response_bytes: usize,
    timeout: Duration,
}

impl ReadBudget {
    pub(crate) fn new(retain_frames: bool) -> Arc<Self> {
        Arc::new(Self {
            retain_frames,
            slots: Arc::new(Semaphore::new(CONCURRENT_READS)),
            observations: Arc::new(Semaphore::new(2)),
            reports: Arc::new(Semaphore::new(2)),
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
    let Some(lane) = lanes::classify(&request) else {
        return next.run(request).await;
    };
    let slots = match lane {
        Lane::Bulk => &budget.slots,
        Lane::Observation => &budget.observations,
        Lane::Report => &budget.reports,
    };
    let Ok(permit) = slots.clone().try_acquire_owned() else {
        use std::sync::atomic::{AtomicU64, Ordering};
        static REFUSALS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
        let refused = REFUSALS[lane as usize]
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);
        if refused.is_power_of_two() {
            tracing::warn!(
                guard = "buffered_reads",
                lane = lane.name(),
                refused,
                "shared read capacity is busy"
            );
        }
        let mut response = private(HttpProblem::BufferedReadCapacityExhausted.into_response());
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, lane.retry_after().parse().unwrap());
        return response;
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
    let byte_limit = byte_limit.min(lane.bytes());
    let timeout = budget.timeout.min(lane.timeout());
    // Refuse before running downstream rate-limit/activity/store/provider I/O.
    // Buffer fully before releasing any bytes, including chunked bodies with
    // misleading or absent Content-Length. Dropped requests release the slot.
    let result = tokio::time::timeout(timeout, async {
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
