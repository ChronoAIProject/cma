//! Real provider HTTP/SSE acceptance, with separate cma_codex and nyxbot peers.
use super::{
    fake_provider::{FakeProvider, FakeProviderConfig},
    fanout_support::*,
    preparation_support::{voice, Media, Speech},
    routing_fixture::NYX,
};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    providers::{ProviderClient, ProviderRegistry},
    turns::{outcomes::DeliveryCommand, TurnRunner},
    TurnConfig,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn runner(h: &Harness, nyx: &FakeProvider) -> TurnRunner {
    let entry = |slug: &str, base: &str| {
        json!({"slug":slug,"base_url":base,"kind":"responses_http",
        "paths":{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{binding_id}",
        "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}","responses":"/api/v2/responses"},"max_inflight":8})
    };
    let registry = ProviderRegistry::parse(
        &json!([
            entry("cma_codex", h.base.fake.base_url()),
            entry("nyxbot", nyx.base_url())
        ])
        .to_string(),
    )
    .unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 8).unwrap(),
    );
    TurnRunner::new(
        providers,
        h.base.tracker.clone(),
        h.delivery.clone(),
        TurnConfig::default(),
    )
}

#[tokio::test]
async fn real_peers_each_receive_one_child_and_share_the_bound_activity() {
    let mut h = Harness::new().await;
    let nyx = FakeProvider::start(Default::default()).await;
    let runner = Arc::new(runner(&h, &nyx));
    let outcome = h
        .wrap(runner, Duration::from_secs(2))
        .launch_routed(h.request(&["default", NYX]), None)
        .await;
    assert!(outcome
        .consumers
        .iter()
        .all(|c| matches!(c.outcome, LaunchOutcome::Accepted { .. })));
    let cma = h.base.fake.requests();
    let nyx_requests = nyx.requests();
    let a = cma.iter().find(|r| r.method == "POST").unwrap();
    let b = nyx_requests.iter().find(|r| r.method == "POST").unwrap();
    let a_body: serde_json::Value = serde_json::from_str(&a.body).unwrap();
    let b_body: serde_json::Value = serde_json::from_str(&b.body).unwrap();
    assert_ne!(a.idempotency_key, b.idempotency_key);
    assert_ne!(a_body["conversation"], b_body["conversation"]);
    assert_ne!(a.authorization, b.authorization);
    while !h.base.tracker.is_empty() {
        tokio::select! { _ = h.outcomes.recv() => {}, _ = tokio::time::sleep(Duration::from_millis(5)) => {} }
    }
    nyx.close().await;
}

#[tokio::test]
async fn routed_provider_rejection_preserves_safe_code_and_legacy_problem() {
    for (provider_code, retained) in [
        ("access_denied", Some("access_denied")),
        ("private detail=/key", None),
    ] {
        let mut h = Harness::new().await;
        let nyx = FakeProvider::start(FakeProviderConfig {
            reject_response: Some((403, provider_code.into())),
            ..Default::default()
        })
        .await;
        let runner = Arc::new(runner(&h, &nyx));
        let outcome = h
            .wrap(runner.clone(), Duration::from_secs(2))
            .launch_routed(h.request(&["default", NYX]), None)
            .await;
        let LaunchOutcome::Refused(refusal) = &outcome.consumers[1].outcome else {
            panic!()
        };
        assert_eq!(refusal.code, "provider_rejected");
        assert_eq!(refusal.provider_code.as_deref(), retained);
        assert_eq!(
            refusal.problem.detail.as_deref(),
            Some("provider_unavailable")
        );
        assert_eq!(refusal.problem.status, 503);
        let observed = h
            .observer
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|o| o.consumer.as_str() == NYX)
            .unwrap()
            .clone();
        assert_eq!(observed.provider_code.as_deref(), retained);
        let mut failures = 0;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let command = h.outcomes.recv().await.unwrap();
                if let DeliveryCommand::ConsumerFailed {
                    code,
                    provider_code,
                    ..
                } = command
                {
                    failures += 1;
                    assert_eq!(code, "provider_rejected");
                    assert_eq!(provider_code.as_deref(), retained);
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(failures, 1);
        let mut legacy = h.request(&[NYX]);
        legacy.activity.routing = None;
        legacy.consumer = Some(cmaeg_core::ids::ConsumerId::parse(NYX).unwrap());
        let LaunchOutcome::Refused(legacy) = runner.launch(legacy).await else {
            panic!()
        };
        assert_eq!(legacy.problem, refusal.problem);
        assert_eq!(legacy.code, "provider_unavailable");
        assert_eq!(legacy.provider_code, None);
        nyx.close().await;
    }
}

#[tokio::test]
async fn two_routed_audio_consumers_share_one_fetch_and_transcription() {
    let mut media = Media::new(Speech::Words, Duration::from_secs(1)).await;
    let items = vec![voice(&media, 0, Some(4))];
    let request = route(
        media.turn("same-audio", items, add_consumers),
        &["default", NYX],
    );
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let launcher = cmaeg::turns::fanout::FanoutLauncher::new(
        Arc::new(media.runner.clone()),
        keys(),
        Arc::new(cmaeg::delivery::router::ShardedScheduler::new(sender)),
        Arc::new(cmaeg::turns::capacity::RoutedCapacity::new(
            Default::default(),
        )),
        media.fixture.tracker.clone(),
        Duration::from_secs(2),
        Arc::new(cmaeg::turns::observer::NoopObserver),
    );
    let outcome = launcher.launch_routed(request, None).await;
    assert!(outcome
        .consumers
        .iter()
        .all(|c| matches!(c.outcome, LaunchOutcome::Accepted { .. })));
    assert_eq!(media.source.calls(), 1);
    assert_eq!(media.words.calls(), 1);
    while !media.fixture.tracker.is_empty() {
        tokio::select! { _ = media.fixture.outcomes.recv() => {}, _ = tokio::time::sleep(Duration::from_millis(5)) => {} }
    }
}
