//! `GET /v1/media/content`: the serving half of single-use media links.
//!
//! The capability travels only in the `Authorization` header, because paths
//! and query strings reach load-balancer and access logs; neither is ever
//! read. Every request re-validates the link's authority, then streams the
//! original under pod-wide and per-channel stream slots, a share of the media
//! response budget and the channel's hourly byte quota, with a hard deadline
//! that releases all of them even when the reader never polls.
use super::problems::{problem, protocol_headers};
use crate::{
    media::{
        body::Overflow,
        forward::{forward_stream, PermitSet, DOWNLOAD_DEADLINE},
        source::{FetchRequest, SourceCredential, SourceStream},
        FetchError,
    },
    telemetry::names,
    tools::{safe_name, Authorized, Refusal, ToolsState, DOWNLOAD_RESPONSE_WEIGHT},
};
use axum::{
    extract::State,
    http::{
        header::{
            AUTHORIZATION, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH,
            CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY, RETRY_AFTER,
            X_CONTENT_TYPE_OPTIONS,
        },
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use cmaeg_core::{
    problem::{Problem, ProblemCode},
    seal::MediaLinkRef,
};
use futures_util::StreamExt;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::Instant;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{field::Empty, Instrument};

/// The longest a download waits for its share of the response budget.
const BUDGET_WAIT: Duration = Duration::from_secs(2);
/// Stream slots free as soon as another download ends.
const BUSY_RETRY_SECONDS: u32 = 2;
/// The byte quota reports no wait time, so a refusal names a fixed minute.
const QUOTA_RETRY_SECONDS: u32 = 60;
const KIB: u64 = 1024;
const SANDBOX_POLICY: &str = "default-src 'none'; sandbox";

/// The download route. It is always mounted, so the documented path never
/// 404s; while media is disabled it answers `503 media_unavailable`.
pub fn router(state: ToolsState) -> Router {
    crate::telemetry::http::instrument(
        Router::new()
            .route("/v1/media/content", get(download_media))
            .with_state(state)
            .layer(RequestBodyLimitLayer::new(0))
            .layer(axum::middleware::from_fn(protocol_headers)),
    )
}

#[utoipa::path(get,path="/v1/media/content",operation_id="downloadMedia",tag="media",
    security(("mediaBearer"=[])),
    responses((status=200,description="the attachment bytes",content_type="application/octet-stream"),
        (status="default",body=Problem,content_type="application/problem+json")))]
pub async fn download_media(State(state): State<ToolsState>, headers: HeaderMap) -> Response {
    let span = tracing::info_span!(
        "cmaeg.media.download",
        channel_hash = Empty,
        outcome = Empty,
        bytes = Empty,
    );
    match serve(&state, &headers, span.clone())
        .instrument(span.clone())
        .await
    {
        Ok(response) => response,
        Err(refusal) => {
            record(&span, refusal.outcome, 0);
            span.in_scope(|| tracing::info!(outcome = refusal.outcome, "media download refused"));
            refused(refusal)
        }
    }
}

/// Every step up to the response head; the body's own outcome is recorded
/// when its forwarding task ends.
async fn serve(
    state: &ToolsState,
    headers: &HeaderMap,
    span: tracing::Span,
) -> Result<Response, Refusal> {
    let Some(media) = state.media.clone() else {
        return Err(Refusal::new(
            "unavailable",
            ProblemCode::MediaUnavailable,
            503,
            "media_disabled",
        ));
    };
    let token = bearer(headers).ok_or_else(|| {
        Refusal::new(
            "invalid",
            ProblemCode::InvalidRequest,
            400,
            "media_link_invalid",
        )
    })?;
    let Authorized {
        channel,
        event,
        activity,
        attachment,
    } = state.authorize_download(&media, &token).await?;
    let slots = state.downloads.acquire(&channel.id).ok_or_else(busy)?;
    let budget = media
        .responses()
        .acquire(DOWNLOAD_RESPONSE_WEIGHT, Instant::now() + BUDGET_WAIT)
        .await
        .ok_or_else(busy)?;
    let cap = media.limits().max_bytes;
    let source = media
        .sources()
        .open(FetchRequest {
            channel: &channel,
            event: &event,
            activity: &activity,
            locator: attachment.source.as_ref(),
            declared_size: attachment.size_bytes,
            // A link never carries the reply token, and needs none.
            credential: SourceCredential::ChannelKey,
            cap,
            overflow: Overflow::Reject,
            deadline: Instant::now() + DOWNLOAD_DEADLINE,
        })
        .await
        .map_err(source_refusal)?;
    // Charged before the first byte is forwarded; an undeclared size counts
    // as the most the stream may carry.
    let length = source
        .content_length
        .or(attachment.size_bytes)
        .unwrap_or(cap);
    let kib = u32::try_from(length.div_ceil(KIB)).unwrap_or(u32::MAX);
    if !media.limiter().download_kib.check_n(&channel.id, kib) {
        drop(source);
        return Err(Refusal::new(
            "rate_limited",
            ProblemCode::RateLimited,
            429,
            "download_quota",
        )
        .retry_after(QUOTA_RETRY_SECONDS));
    }
    let content_length = source.content_length;
    let forwarded = Arc::new(AtomicU64::new(0));
    let counter = forwarded.clone();
    let stream = SourceStream {
        content_length,
        stream: source
            .stream
            .inspect(move |chunk| {
                if let Ok(chunk) = chunk {
                    counter.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                }
            })
            .boxed(),
    };
    let mut permits = PermitSet::new();
    permits.push(slots.global);
    permits.push(slots.channel);
    permits.push(budget);
    let body = forward_stream(
        stream,
        permits,
        Instant::now() + DOWNLOAD_DEADLINE,
        move |completed| {
            let outcome = if completed { "served" } else { "aborted" };
            let bytes = forwarded.load(Ordering::Relaxed);
            record(&span, outcome, bytes);
            span.in_scope(|| {
                if completed {
                    tracing::debug!(outcome, bytes, "media download served");
                } else {
                    tracing::info!(outcome, bytes, "media download aborted");
                }
            });
        },
    );
    Ok(hardened(
        body.into_response(),
        &safe_name(attachment.filename.as_deref()),
        content_length,
    ))
}

/// Exactly one `Authorization: Bearer <link>` header. The path, the query and
/// every other header are never read, so a link anywhere else is ignored.
fn bearer(headers: &HeaderMap) -> Option<MediaLinkRef> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.to_str().ok()?.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    token.parse().ok()
}

/// The response head of an original: never sniffed, rendered, cached or
/// referred, and always saved as a file.
fn hardened(mut response: Response, name: &str, content_length: Option<u64>) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    // `safe_name` keeps only header-safe characters, so this cannot fail.
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        headers.insert(CONTENT_DISPOSITION, disposition);
    }
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(SANDBOX_POLICY),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if let Some(length) = content_length {
        headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    }
    response
}

fn busy() -> Refusal {
    Refusal::new("busy", ProblemCode::MediaUnavailable, 503, "download_busy")
        .retry_after(BUSY_RETRY_SECONDS)
}

/// A source failure before any header is sent, as a fixed classification.
fn source_refusal(error: FetchError) -> Refusal {
    match error {
        FetchError::TooLarge => Refusal::new(
            "too_large",
            ProblemCode::PayloadTooLarge,
            413,
            "attachment_too_large",
        ),
        FetchError::NotFound => Refusal::new(
            "not_found",
            ProblemCode::AttachmentNotFound,
            404,
            "attachment_not_found",
        ),
        FetchError::RateLimited => Refusal::new(
            "rate_limited",
            ProblemCode::RateLimited,
            429,
            "source_rate_limited",
        ),
        FetchError::NotRetrievable
        | FetchError::Unauthorized
        | FetchError::TimedOut
        | FetchError::Upstream
        | FetchError::Rejected
        | FetchError::Busy => Refusal::new(
            "unavailable",
            ProblemCode::MediaUnavailable,
            503,
            "source_unavailable",
        ),
    }
}

fn refused(refusal: Refusal) -> Response {
    let Problem {
        code,
        status,
        detail,
        ..
    } = refusal.problem;
    let mut response = problem(
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        code,
        detail.unwrap_or_default(),
    );
    if let Some(seconds) = refusal.retry_after {
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from(seconds));
    }
    response
}

/// One download's outcome: the counter and the span's final fields.
fn record(span: &tracing::Span, outcome: &'static str, bytes: u64) {
    span.record("outcome", outcome);
    span.record("bytes", bytes);
    metrics::counter!(names::MEDIA_DOWNLOADS, names::OUTCOME => outcome).increment(1);
}
