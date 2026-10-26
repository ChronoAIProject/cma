//! Deadline-bounded response bodies, under paused time.
use bytes::Bytes;
use cmaeg::media::{
    body::SourceStream,
    forward::{forward_bytes, forward_stream, PermitSet},
    FetchError,
};
use futures_util::StreamExt;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};

/// A one-permit semaphore whose permit rides in the set, so the test can see
/// exactly when the body's task releases it.
fn held() -> (Arc<Semaphore>, PermitSet) {
    let semaphore = Arc::new(Semaphore::new(1));
    let mut permits = PermitSet::new();
    permits.push(semaphore.clone().try_acquire_owned().unwrap());
    assert!(!permits.is_empty());
    (semaphore, permits)
}

fn payload(length: usize) -> Bytes {
    Bytes::from((0..length).map(|index| index as u8).collect::<Vec<_>>())
}

/// What a body's `on_end` reported, if it ran yet.
type Ended = Arc<Mutex<Option<bool>>>;

fn outcome() -> (Ended, impl FnOnce(bool) + Send + 'static) {
    let ended = Ended::default();
    let report = ended.clone();
    (ended, move |completed| {
        *report.lock().unwrap() = Some(completed);
    })
}

async fn settle() {
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn permits_are_released_after_the_last_chunk() {
    let (semaphore, permits) = held();
    let sent = payload(200 * 1024 + 5);
    let body = forward_bytes(
        sent.clone(),
        permits,
        Instant::now() + Duration::from_secs(15),
    );
    let mut stream = body.into_data_stream();
    let mut received = Vec::new();
    let mut sizes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        sizes.push(chunk.len());
        received.extend_from_slice(&chunk);
    }
    assert_eq!(received, sent);
    assert_eq!(sizes, [65_536, 65_536, 65_536, 8_197]);
    assert_eq!(semaphore.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn permits_are_released_when_the_receiver_is_dropped() {
    let (semaphore, permits) = held();
    let body = forward_bytes(
        payload(1024 * 1024),
        permits,
        Instant::now() + Duration::from_secs(15),
    );
    let mut stream = body.into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().len(), 65_536);
    assert_eq!(semaphore.available_permits(), 0);
    drop(stream);
    settle().await;
    assert_eq!(semaphore.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn permits_are_released_at_the_deadline_while_the_receiver_is_never_polled() {
    let (semaphore, permits) = held();
    let body = forward_bytes(
        payload(1024 * 1024),
        permits,
        Instant::now() + Duration::from_secs(15),
    );
    tokio::time::sleep(Duration::from_secs(14)).await;
    assert_eq!(semaphore.available_permits(), 0);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(semaphore.available_permits(), 1);
    // The one chunk already queued is still delivered, then the body fails
    // instead of ending as if it were complete.
    let mut stream = body.into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().len(), 65_536);
    assert!(stream.next().await.unwrap().is_err());
    assert!(stream.next().await.is_none());
}

fn source(chunks: Vec<Result<Bytes, FetchError>>, then_wait: bool) -> SourceStream {
    let items = futures_util::stream::iter(chunks);
    let stream = if then_wait {
        items.chain(futures_util::stream::pending()).boxed()
    } else {
        items.boxed()
    };
    SourceStream {
        content_length: None,
        stream,
    }
}

#[tokio::test(start_paused = true)]
async fn forwarded_streams_are_split_into_bounded_chunks() {
    let (semaphore, permits) = held();
    let (ended, on_end) = outcome();
    let body = forward_stream(
        source(vec![Ok(payload(150 * 1024)), Ok(payload(10))], false),
        permits,
        Instant::now() + Duration::from_secs(15),
        on_end,
    );
    let sizes: Vec<usize> = body
        .into_data_stream()
        .map(|chunk| chunk.unwrap().len())
        .collect()
        .await;
    assert_eq!(sizes, [65_536, 65_536, 22_528, 10]);
    assert_eq!(semaphore.available_permits(), 1);
    // Reported before the body ended, so a reader that saw the end can rely on it.
    assert_eq!(*ended.lock().unwrap(), Some(true));
}

#[tokio::test(start_paused = true)]
async fn an_upstream_error_aborts_the_forwarded_body() {
    let (semaphore, permits) = held();
    let (ended, on_end) = outcome();
    let body = forward_stream(
        source(vec![Ok(payload(10)), Err(FetchError::TooLarge)], false),
        permits,
        Instant::now() + Duration::from_secs(15),
        on_end,
    );
    let mut stream = body.into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().len(), 10);
    assert!(stream.next().await.unwrap().is_err());
    assert_eq!(semaphore.available_permits(), 1);
    assert_eq!(*ended.lock().unwrap(), Some(false));
}

#[tokio::test(start_paused = true)]
async fn a_quiet_upstream_still_releases_when_the_reader_leaves_or_the_deadline_passes() {
    let (left, permits) = held();
    let body = forward_stream(
        source(vec![Ok(payload(10))], true),
        permits,
        Instant::now() + Duration::from_secs(300),
        |_| {},
    );
    let mut stream = body.into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().len(), 10);
    drop(stream);
    settle().await;
    assert_eq!(left.available_permits(), 1);

    let (expired, permits) = held();
    let (ended, on_end) = outcome();
    let body = forward_stream(
        source(Vec::new(), true),
        permits,
        Instant::now() + Duration::from_secs(15),
        on_end,
    );
    tokio::time::sleep(Duration::from_secs(16)).await;
    assert_eq!(expired.available_permits(), 1);
    assert_eq!(*ended.lock().unwrap(), Some(false));
    assert!(body.into_data_stream().next().await.unwrap().is_err());
}

#[tokio::test(start_paused = true)]
async fn an_empty_body_ends_cleanly() {
    let (semaphore, permits) = held();
    let body = forward_bytes(
        Bytes::new(),
        permits,
        Instant::now() + Duration::from_secs(15),
    );
    assert!(body.into_data_stream().next().await.is_none());
    assert_eq!(semaphore.available_permits(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_declared_length_completes_without_waiting_for_the_upstream_end() {
    let (semaphore, permits) = held();
    let (ended, on_end) = outcome();
    let mut quiet = source(vec![Ok(payload(70 * 1024))], true);
    quiet.content_length = Some(70 * 1024);
    let body = forward_stream(
        quiet,
        permits,
        Instant::now() + Duration::from_secs(300),
        on_end,
    );
    let sizes: Vec<usize> = body
        .into_data_stream()
        .map(|chunk| chunk.unwrap().len())
        .collect()
        .await;
    assert_eq!(sizes, [65_536, 6_144]);
    assert_eq!(semaphore.available_permits(), 1);
    assert_eq!(*ended.lock().unwrap(), Some(true));
}
