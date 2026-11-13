//! Admit public observers before handler work; retain slots through body lifetime.
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{MatchedPath, Request, State},
    http::{header, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::{stream, StreamExt};
use http_body_util::{BodyStream, StreamBody};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) struct StreamBudget {
    slots: Option<Arc<Semaphore>>,
}

impl StreamBudget {
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            slots: (limit > 0).then(|| Arc::new(Semaphore::new(limit))),
        })
    }
}

pub(crate) async fn enforce(
    State(budget): State<Arc<StreamBudget>>,
    request: Request,
    next: Next,
) -> Response {
    let Some(slots) = budget.slots.as_ref().filter(|_| candidate(&request)) else {
        return next.run(request).await;
    };
    let Ok(permit) = slots.clone().try_acquire_owned() else {
        return refused();
    };
    // Acquired before body parsing, history projection or command admission.
    // Cancellation/panic before headers drops the permit with this future.
    let response = next.run(request).await;
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, retain(body, permit))
}

fn retain(body: Body, permit: OwnedSemaphorePermit) -> Body {
    // Preserve data, trailers and errors, without collecting or prefetching frames.
    let frames = stream::unfold(Some((BodyStream::new(body), permit)), |state| async move {
        let (mut body, permit) = state?;
        let frame = body.next().await?;
        let state = frame.is_ok().then_some((body, permit));
        Some((frame, state))
    });
    Body::new(StreamBody::new(frames))
}

fn candidate(request: &Request) -> bool {
    // Only router-selected templates qualify. URI suffixes cannot grant or
    // bypass admission, and no request-body guess decides whether it streams.
    let Some(path) = request.extensions().get::<MatchedPath>() else {
        return false;
    };
    match *request.method() {
        Method::GET | Method::HEAD => matches!(
            path.as_str(),
            "/api/v1/agui/events"
                | "/api/v1/agui/launches/{id}/events"
                | "/api/v2/agents/{agent_id}/events"
                | "/api/v2/responses/{response_id}/events"
        ),
        Method::POST => matches!(
            path.as_str(),
            "/api/v1/agui/run"
                | "/api/v1/responses"
                | "/api/v2/responses"
                | "/api/v2/agents/{agent_id}/responses"
        ),
        _ => false,
    }
}

fn refused() -> Response {
    let mut response =
        layer1_public::rest::HttpProblem::PublicStreamCapacityExhausted.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    headers.append(header::LINK, crate::REST_LINKS.clone());
    response
}

#[cfg(test)]
mod tests;
