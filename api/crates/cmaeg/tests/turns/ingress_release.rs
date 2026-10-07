//! How long a launched turn holds the shared ingress permit, and proof that
//! releasing it early keeps every transport's acceptance semantics.
use super::{
    fake_provider::FakeProviderConfig,
    media_ingress::{custom_event, native_update, photo, relay_message, MediaApp, Transport},
    support::Fixture,
};
use cmaeg::{
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    IngressConfig, TurnConfig,
};
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Semaphore;

/// Records how many permits were free while `launch` ran.
struct Observer {
    semaphore: Arc<Semaphore>,
    seen: Mutex<Vec<usize>>,
}

impl TurnLauncher for Observer {
    fn launch(&self, _request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(async move {
            tokio::task::yield_now().await;
            self.seen
                .lock()
                .unwrap()
                .push(self.semaphore.available_permits());
            LaunchOutcome::Deferred
        })
    }
}

#[tokio::test]
async fn default_launch_holding_keeps_the_permit_until_launch_returns() {
    let fixture = Fixture::new(FakeProviderConfig::default(), TurnConfig::default(), 1).await;
    let semaphore = Arc::new(Semaphore::new(2));
    let observer = Observer {
        semaphore: semaphore.clone(),
        seen: Mutex::default(),
    };
    let permit = semaphore.clone().try_acquire_owned().unwrap();
    assert_eq!(semaphore.available_permits(), 1);
    let outcome = observer
        .launch_holding(fixture.request("hold-default"), permit)
        .await;
    assert!(matches!(outcome, LaunchOutcome::Deferred));
    assert_eq!(*observer.seen.lock().unwrap(), [1], "held during launch");
    assert_eq!(semaphore.available_permits(), 2, "released after launch");
    fixture.fake.close().await;
}

/// Acceptance arrives `delay` after each provider request.
async fn delayed(delay: Duration) -> Fixture {
    let config = FakeProviderConfig {
        response_delay: delay,
        ..Default::default()
    };
    Fixture::new(config, TurnConfig::default(), 8).await
}

/// Wait until `semaphore` has `permits` free, or fail after `within`.
async fn until_available(semaphore: &Semaphore, permits: usize, within: Duration) {
    tokio::time::timeout(within, async {
        while semaphore.available_permits() != permits {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{} permits free", semaphore.available_permits()));
}

#[tokio::test]
async fn media_turn_releases_permit_after_spawn() {
    let fixture = delayed(Duration::from_secs(2)).await;
    let semaphore = Arc::new(Semaphore::new(4));
    let mut launches = Vec::new();
    for index in 0..4 {
        let permit = semaphore.clone().try_acquire_owned().unwrap();
        let request = fixture.request_with_kind(&format!("media-{index}"), Some(photo()));
        let runner = fixture.runner.clone();
        launches.push(tokio::spawn(async move {
            runner.launch_holding(request, permit).await
        }));
    }
    until_available(&semaphore, 4, Duration::from_secs(1)).await;
    assert!(
        launches.iter().all(|launch| !launch.is_finished()),
        "acceptance waits are still pending"
    );
    for launch in launches {
        assert!(matches!(
            launch.await.unwrap(),
            LaunchOutcome::Accepted { .. }
        ));
    }
    fixture.fake.close().await;
}

#[tokio::test]
async fn text_turn_keeps_permit_through_wait() {
    let fixture = delayed(Duration::from_secs(1)).await;
    let semaphore = Arc::new(Semaphore::new(1));
    let permit = semaphore.clone().try_acquire_owned().unwrap();
    let request = fixture.request("text-hold");
    let runner = fixture.runner.clone();
    let launch = tokio::spawn(async move { runner.launch_holding(request, permit).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!launch.is_finished());
    assert_eq!(semaphore.available_permits(), 0, "held through the wait");
    assert_eq!(fixture.runner.sequencer().live_chains(), 0);
    assert!(matches!(
        launch.await.unwrap(),
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(semaphore.available_permits(), 1);
    fixture.fake.close().await;
}

fn ingress(max_inflight: usize) -> IngressConfig {
    IngressConfig {
        max_inflight,
        ..Default::default()
    }
}

/// Media turns' provider starts take `delay`; text turns start at once.
fn media_delayed(delay: Duration) -> FakeProviderConfig {
    FakeProviderConfig {
        attachment_delay: delay,
        ..Default::default()
    }
}

async fn status_of(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn relay_media_burst_does_not_429_text() {
    let app = Arc::new(
        MediaApp::new(
            Transport::Relay,
            media_delayed(Duration::from_secs(10)),
            TurnConfig::default(),
            ingress(4),
        )
        .await,
    );
    let mut media = Vec::new();
    for sender in 1..=4 {
        let app = app.clone();
        let signed = relay_message(sender, true);
        media.push(tokio::spawn(async move {
            status_of(app.relay(&signed).await).await
        }));
    }
    app.wait_for_turns(4).await;
    let (status, body) = status_of(app.relay(&relay_message(99, false)).await).await;
    assert_eq!((status, body["status"].as_str()), (202, Some("accepted")));
    assert!(
        media.iter().all(|handler| !handler.is_finished()),
        "the media handlers still wait"
    );
    for handler in media {
        let (status, body) = handler.await.unwrap();
        assert_eq!((status, body["status"].as_str()), (202, Some("accepted")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn burst_of_300_media_callbacks_never_429s_text() {
    let app = Arc::new(
        MediaApp::new(
            Transport::Relay,
            media_delayed(Duration::from_secs(10)),
            TurnConfig::default(),
            ingress(256),
        )
        .await,
    );
    // Warm the channel and key caches so the burst measures admission only.
    let (status, _) = status_of(app.relay(&relay_message(1_000, false)).await).await;
    assert_eq!(status, 202);
    let signed: Vec<_> = (1..=300)
        .map(|sender| relay_message(sender, true))
        .collect();
    let mut media = Vec::new();
    for signed in signed {
        let app = app.clone();
        media.push(tokio::spawn(async move {
            app.relay(&signed).await.status().as_u16()
        }));
    }
    app.wait_for_turns(301).await;
    until_available(&app.ingress.inflight, 256, Duration::from_secs(5)).await;
    assert!(
        media.iter().all(|handler| !handler.is_finished()),
        "every media acceptance wait is still pending"
    );
    let text = app
        .relay(&relay_message(2_000, false))
        .await
        .status()
        .as_u16();
    let mut statuses = vec![text];
    for handler in media {
        statuses.push(handler.await.unwrap());
    }
    assert_eq!(statuses.iter().filter(|status| **status == 429).count(), 0);
    assert!(statuses.iter().all(|status| *status == 202), "{statuses:?}");
}

/// A turn configuration whose acceptance budget and busy retries are short.
fn short(accept_ms: u64, busy_ms: u64) -> TurnConfig {
    TurnConfig {
        accept_budget: Duration::from_millis(accept_ms),
        busy_retry_max: Duration::from_millis(busy_ms),
        ..Default::default()
    }
}

#[tokio::test]
async fn media_callback_is_accepted_like_text() {
    let provider = FakeProviderConfig {
        header_delay: Duration::from_secs(2),
        ..Default::default()
    };
    let app = MediaApp::new(
        Transport::Relay,
        provider,
        TurnConfig::default(),
        ingress(4),
    )
    .await;
    for photo in [true, false] {
        let started = std::time::Instant::now();
        let (status, body) = status_of(app.relay(&relay_message(7, photo)).await).await;
        assert_eq!((status, body["status"].as_str()), (202, Some("accepted")));
        assert!(started.elapsed() >= Duration::from_secs(2), "{photo}");
    }
}

#[tokio::test]
async fn media_callback_past_the_acceptance_budget_is_deferred_like_text() {
    let provider = FakeProviderConfig {
        header_delay: Duration::from_secs(3),
        ..Default::default()
    };
    let app = MediaApp::new(
        Transport::Relay,
        provider,
        short(1_000, 180_000),
        ingress(4),
    )
    .await;
    for (sender, photo) in [(7, true), (8, false)] {
        let started = std::time::Instant::now();
        let (status, body) = status_of(app.relay(&relay_message(sender, photo)).await).await;
        assert_eq!((status, body["status"].as_str()), (202, Some("accepted")));
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_secs(1), "{photo}: {elapsed:?}");
        assert!(
            elapsed < Duration::from_secs(3),
            "deferred, {photo}: {elapsed:?}"
        );
    }
}

#[tokio::test]
async fn busy_custom_media_event_answers_503_like_text() {
    let provider = FakeProviderConfig {
        busy_always: true,
        ..Default::default()
    };
    let app = MediaApp::new(
        Transport::Custom,
        provider,
        short(20_000, 1_000),
        ingress(4),
    )
    .await;
    for (id, media) in [("busy-media", true), ("busy-text", false)] {
        let response = app.custom(&custom_event(id, media)).await;
        assert_eq!(response.status(), 503, "{id}");
        assert_eq!(response.headers()["retry-after"], "2", "{id}");
        let problem: Value = response.json().await.unwrap();
        assert_eq!(problem["code"], "provider_busy", "{id}: {problem}");
    }
    let turns = app.turns();
    assert!(turns
        .iter()
        .any(|turn| turn.body.contains("[Attachments: 1")));
}

#[tokio::test]
async fn refused_native_media_update_answers_503_like_text() {
    let provider = FakeProviderConfig {
        busy_always: true,
        ..Default::default()
    };
    let app = MediaApp::new(Transport::Native, provider, short(20_000, 1), ingress(4)).await;
    for photo in [true, false] {
        let response = app.native(&native_update(photo)).await;
        assert_eq!(response.status(), 503, "{photo}");
        assert_eq!(response.headers()["retry-after"], "2", "{photo}");
    }
    let turns = app.turns();
    assert!(turns
        .iter()
        .any(|turn| turn.body.contains("[Attachments: 1")));
}

#[tokio::test]
async fn media_turn_at_tracker_capacity_answers_turn_capacity_like_text() {
    let app = MediaApp::new(
        Transport::Relay,
        FakeProviderConfig::default(),
        TurnConfig::default(),
        ingress(4),
    )
    .await;
    let source = Fixture::new(FakeProviderConfig::default(), TurnConfig::default(), 1).await;
    let filler = source.request("filler");
    for _ in 0..4_096 {
        app.tracker
            .spawn(&filler, std::future::pending::<()>())
            .unwrap();
    }
    for photo in [true, false] {
        let response = app.relay(&relay_message(7, photo)).await;
        assert_eq!(response.status(), 503, "{photo}");
        let problem: Value = response.json().await.unwrap();
        assert_eq!(problem["detail"], "turn_capacity", "{photo}: {problem}");
    }
    assert!(app.turns().is_empty());
    assert_eq!(app.runner.sequencer().live_chains(), 0);
    source.fake.close().await;
}
