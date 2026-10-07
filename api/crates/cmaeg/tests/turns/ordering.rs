//! Best-effort provider start order within one conversation.
use super::{fake_provider::FakeProviderConfig, media_ingress::photo, support::Fixture};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    telemetry::timing::{IngressTiming, MediaLabel},
    TurnConfig,
};
use std::time::Duration;

/// Media turns' provider starts take `delay`; text turns start at once.
async fn fixture(delay: Duration) -> Fixture {
    let config = FakeProviderConfig {
        attachment_delay: delay,
        ..Default::default()
    };
    Fixture::new(config, TurnConfig::default(), 8).await
}

/// The provider turn requests, oldest first, as (input contains media, time).
fn arrivals(fixture: &Fixture) -> Vec<(bool, std::time::Instant)> {
    fixture
        .fake
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST")
        .map(|request| (request.body.contains("[Attachments:"), request.received_at))
        .collect()
}

#[tokio::test]
async fn media_then_text_same_conversation_in_order() {
    let fixture = fixture(Duration::from_secs(1)).await;
    let mut media = fixture.request_with_kind("ordered-media", Some(photo()));
    let mut text = fixture.request("ordered-text");
    text.conversation_id = media.conversation_id.clone();
    let (media_timing, text_timing) = (IngressTiming::new("custom"), IngressTiming::new("custom"));
    media.timing = Some(media_timing.clone());
    text.timing = Some(text_timing.clone());
    let runner = fixture.runner.clone();
    let first = tokio::spawn(async move { runner.launch(media).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(fixture.runner.sequencer().live_chains(), 1);
    let runner = fixture.runner.clone();
    let second = tokio::spawn(async move { runner.launch(text).await });
    assert!(matches!(
        first.await.unwrap(),
        LaunchOutcome::Accepted { .. }
    ));
    assert!(matches!(
        second.await.unwrap(),
        LaunchOutcome::Accepted { .. }
    ));
    let arrivals = arrivals(&fixture);
    assert_eq!(
        arrivals.iter().map(|(media, _)| *media).collect::<Vec<_>>(),
        [true, false],
        "the media turn reaches the provider first"
    );
    // The text turn waited for the media turn's start to return.
    assert!(arrivals[1].1 - arrivals[0].1 >= Duration::from_millis(900));
    assert_eq!(
        media_timing.media(),
        MediaLabel::None,
        "nothing was prepared"
    );
    assert_eq!(text_timing.media(), MediaLabel::Fenced);
    assert_eq!(fixture.runner.sequencer().live_chains(), 0);
    fixture.fake.close().await;
}

#[tokio::test]
async fn different_conversations_are_not_ordered() {
    let fixture = fixture(Duration::from_secs(1)).await;
    let media = fixture.request_with_kind("unordered-media", Some(photo()));
    let text = fixture.request("unordered-text");
    assert_ne!(media.conversation_id, text.conversation_id);
    let runner = fixture.runner.clone();
    let first = tokio::spawn(async move { runner.launch(media).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let outcome = fixture.runner.launch(text).await;
    assert!(matches!(outcome, LaunchOutcome::Accepted { .. }));
    assert!(!first.is_finished(), "the text turn did not wait for media");
    assert!(matches!(
        first.await.unwrap(),
        LaunchOutcome::Accepted { .. }
    ));
    let arrivals = arrivals(&fixture);
    assert!(arrivals[1].1 - arrivals[0].1 < Duration::from_millis(500));
    fixture.fake.close().await;
}

#[tokio::test]
async fn text_only_creates_no_chain() {
    let fixture = fixture(Duration::ZERO).await;
    let mut launches = Vec::new();
    for index in 0..4 {
        let mut request = fixture.request(&format!("text-only-{index}"));
        if index > 0 {
            // One conversation, so any chain would be visible.
            request.conversation_id = fixture.request("text-only-0").conversation_id;
        }
        let runner = fixture.runner.clone();
        launches.push(tokio::spawn(async move { runner.launch(request).await }));
    }
    while launches.iter().any(|launch| !launch.is_finished()) {
        assert_eq!(fixture.runner.sequencer().live_chains(), 0);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    for launch in launches {
        assert!(matches!(
            launch.await.unwrap(),
            LaunchOutcome::Accepted { .. }
        ));
    }
    assert_eq!(fixture.runner.sequencer().live_chains(), 0);
    fixture.fake.close().await;
}

#[tokio::test]
async fn chain_entry_removed_after_release() {
    let fixture = fixture(Duration::from_millis(500)).await;
    let media = fixture.request_with_kind("released-media", Some(photo()));
    let runner = fixture.runner.clone();
    let launch = tokio::spawn(async move { runner.launch(media).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        fixture.runner.sequencer().live_chains(),
        1,
        "live while starting"
    );
    assert!(matches!(
        launch.await.unwrap(),
        LaunchOutcome::Accepted { .. }
    ));
    assert_eq!(
        fixture.runner.sequencer().live_chains(),
        0,
        "gone once started"
    );
    fixture.fake.close().await;
}
