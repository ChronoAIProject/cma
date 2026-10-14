//! Byte-capped body reads against a loopback server whose routes count the
//! body bytes they produce.
use super::support::{far, serve};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header::CONTENT_LENGTH, Response},
    routing::get,
    Router,
};
use bytes::Bytes;
use cmaeg::media::{
    body::{read_capped, stream_capped, Overflow},
    FetchError,
};
use futures_util::StreamExt;
use std::{
    convert::Infallible,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::time::Instant;

const CHUNK: usize = 8 * 1024;

#[derive(Clone, Default)]
struct Produced(Arc<AtomicUsize>);

/// Routes: a declared length with a body that never arrives, a streamed body
/// without a declared length, a body of exactly its declared length, and a
/// body that stalls after its first chunk.
async fn server() -> (String, Produced) {
    let produced = Produced::default();
    let router = Router::new()
        .route("/declared/{length}", get(declared))
        .route("/stream/{length}", get(streamed))
        .route("/exact/{length}", get(exact))
        .route("/stall", get(stall))
        .with_state(produced.clone());
    (serve(router).await, produced)
}

async fn declared(Path(length): Path<u64>) -> Response<Body> {
    Response::builder()
        .header(CONTENT_LENGTH, length)
        .body(Body::from_stream(futures_util::stream::pending::<
            Result<Bytes, Infallible>,
        >()))
        .unwrap()
}

async fn streamed(State(produced): State<Produced>, Path(length): Path<usize>) -> Body {
    Body::from_stream(chunks(length, produced))
}

async fn exact(State(produced): State<Produced>, Path(length): Path<usize>) -> Body {
    produced.0.fetch_add(length, Ordering::SeqCst);
    Body::from(vec![b'x'; length])
}

async fn stall() -> Body {
    Body::from_stream(
        futures_util::stream::once(async { Ok::<_, Infallible>(Bytes::from_static(b"first")) })
            .chain(futures_util::stream::pending()),
    )
}

fn chunks(
    length: usize,
    produced: Produced,
) -> impl futures_util::Stream<Item = Result<Bytes, Infallible>> {
    futures_util::stream::iter((0..length).step_by(CHUNK)).map(move |start| {
        let size = CHUNK.min(length - start);
        produced.0.fetch_add(size, Ordering::SeqCst);
        Ok(Bytes::from(vec![b'x'; size]))
    })
}

async fn get_response(base: &str, path: &str) -> reqwest::Response {
    reqwest::get(format!("{base}{path}")).await.unwrap()
}

#[tokio::test]
async fn a_declared_length_over_the_cap_is_too_large_before_any_body_byte() {
    let (base, produced) = server().await;
    let started = Instant::now();
    let response = get_response(&base, "/declared/2048").await;
    let error = read_capped(response, 1024, Overflow::Reject, far())
        .await
        .unwrap_err();
    assert_eq!(error, FetchError::TooLarge);
    // A reader would have waited on the never-arriving body until the deadline.
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(produced.0.load(Ordering::SeqCst), 0);

    // The same holds for a body source that counts every poll.
    let polls = Arc::new(AtomicUsize::new(0));
    let counted = polls.clone();
    let source = futures_util::stream::poll_fn(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        std::task::Poll::Ready(Some(Ok::<_, Infallible>(Bytes::from_static(b"x"))))
    });
    let response = reqwest::Response::from(
        Response::builder()
            .header(CONTENT_LENGTH, 2048)
            .body(reqwest::Body::wrap_stream(source))
            .unwrap(),
    );
    assert_eq!(
        read_capped(response, 1024, Overflow::Reject, far())
            .await
            .unwrap_err(),
        FetchError::TooLarge
    );
    assert_eq!(polls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_streamed_overflow_without_a_declared_length_is_too_large() {
    let (base, _) = server().await;
    let response = get_response(&base, "/stream/40000").await;
    assert_eq!(
        read_capped(response, 20_000, Overflow::Reject, far())
            .await
            .unwrap_err(),
        FetchError::TooLarge
    );
}

#[tokio::test]
async fn bodies_within_the_cap_are_read_whole() {
    let (base, _) = server().await;
    for path in ["/stream/20000", "/exact/20000"] {
        let response = get_response(&base, path).await;
        let body = read_capped(response, 20_000, Overflow::Reject, far())
            .await
            .unwrap();
        assert_eq!(body.bytes.len(), 20_000, "{path}");
        assert!(!body.truncated);
    }
}

#[tokio::test]
async fn the_deadline_is_timed_out() {
    let (base, _) = server().await;
    let response = get_response(&base, "/stall").await;
    let deadline = Instant::now() + Duration::from_millis(200);
    assert_eq!(
        read_capped(response, 1024, Overflow::Reject, deadline)
            .await
            .unwrap_err(),
        FetchError::TimedOut
    );
}

#[tokio::test]
async fn truncate_keeps_exactly_the_cap() {
    let (base, _) = server().await;
    for path in ["/stream/40000", "/exact/40000"] {
        let response = get_response(&base, path).await;
        let body = read_capped(response, 10_000, Overflow::Truncate, far())
            .await
            .unwrap();
        assert_eq!(body.bytes.len(), 10_000, "{path}");
        assert!(body.truncated, "{path}");
    }
}

#[tokio::test]
async fn a_body_longer_than_its_declared_length_is_upstream() {
    let response = reqwest::Response::from(
        Response::builder()
            .header(CONTENT_LENGTH, 4)
            .body(reqwest::Body::wrap_stream(futures_util::stream::iter([
                Ok::<_, Infallible>(Bytes::from_static(b"1234")),
                Ok(Bytes::from_static(b"5678")),
            ])))
            .unwrap(),
    );
    assert_eq!(
        read_capped(response, 1024, Overflow::Reject, far())
            .await
            .unwrap_err(),
        FetchError::Upstream
    );
}

#[tokio::test]
async fn a_body_cut_short_of_its_declared_length_is_upstream() {
    let response = reqwest::Response::from(
        Response::builder()
            .header(CONTENT_LENGTH, 8)
            .body(reqwest::Body::wrap_stream(futures_util::stream::iter([
                Ok::<_, Infallible>(Bytes::from_static(b"1234")),
            ])))
            .unwrap(),
    );
    assert_eq!(
        read_capped(response, 1024, Overflow::Truncate, far())
            .await
            .unwrap_err(),
        FetchError::Upstream
    );
}

#[tokio::test]
async fn streams_refuse_a_declared_overflow_up_front() {
    let (base, produced) = server().await;
    let response = get_response(&base, "/declared/2048").await;
    assert_eq!(
        stream_capped(response, 1024).unwrap_err(),
        FetchError::TooLarge
    );
    assert_eq!(produced.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn streams_end_with_too_large_at_the_first_chunk_crossing_the_cap() {
    let (base, _) = server().await;
    let response = get_response(&base, "/stream/40000").await;
    let mut source = stream_capped(response, 20_000).unwrap();
    assert_eq!(source.content_length, None);
    let mut received = 0;
    let mut errors = Vec::new();
    while let Some(item) = source.stream.next().await {
        match item {
            Ok(chunk) => received += chunk.len(),
            Err(error) => errors.push(error),
        }
    }
    assert!(received <= 20_000);
    assert_eq!(errors, [FetchError::TooLarge]);
}

#[tokio::test]
async fn streams_within_the_cap_end_cleanly() {
    let (base, _) = server().await;
    let response = get_response(&base, "/exact/20000").await;
    let source = stream_capped(response, 20_000).unwrap();
    assert_eq!(source.content_length, Some(20_000));
    let chunks: Vec<_> = source.stream.collect().await;
    assert_eq!(
        chunks
            .into_iter()
            .map(|chunk| chunk.unwrap().len())
            .sum::<usize>(),
        20_000
    );
}
