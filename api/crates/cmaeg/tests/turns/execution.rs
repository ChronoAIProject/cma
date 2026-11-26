use super::{fake_provider::FakeProviderConfig, support::Fixture};
use cmaeg::{
    ingress::launch::{LaunchOutcome, TurnLauncher},
    turns::outcomes::TurnOutcome,
    TurnConfig,
};
use cmaeg_core::{ids::ConsumerId, problem::ProblemCode};
use std::time::{Duration, Instant};

#[tokio::test]
async fn accepted_turn_has_bounded_metadata_and_completes() {
    let mut fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let request = fixture.request("event-1");
    let event_ref = request.event_ref.as_str().to_owned();
    assert!(
        matches!(fixture.runner.launch(request).await,LaunchOutcome::Accepted {response_id} if response_id=="resp_fake")
    );
    let outcomes = fixture.terminal().await;
    assert!(
        matches!(outcomes.last(),Some(TurnOutcome::Completed {final_text:Some(text)}) if text=="hello")
    );
    let requests = fixture.fake.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].idempotency_key.as_deref(), Some("evt_event-1"));
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer runtime-test-provider-bearer")
    );
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).unwrap();
    assert_eq!(body["metadata"]["cmaeg.event"], "event-1");
    assert_eq!(
        body["metadata"]["cmaeg.channel"].as_str().unwrap().len(),
        16
    );
    assert_eq!(
        body["metadata"]["cmaeg.partition"].as_str().unwrap().len(),
        16
    );
    assert_eq!(body["metadata"]["cmaeg.actor"], "user-1");
    assert!(body["metadata"]
        .as_object()
        .unwrap()
        .values()
        .all(|v| v.as_str().unwrap().len() <= 512));
    assert!(event_ref.len() > 512);
    assert!(body["metadata"].get("cmaeg.event_ref").is_none());
    assert!(!requests[0].body.contains("runtime-test-provider-bearer"));
    fixture.fake.close().await;
}

#[tokio::test]
async fn interrupted_accepted_turn_reads_once_or_reattaches_to_its_existing_response() {
    for (status, mismatch) in [
        ("completed", false),
        ("in_progress", false),
        ("completed", true),
    ] {
        let mut fixture = Fixture::new(
            FakeProviderConfig {
                disconnect_after: Some(2),
                recovery_status: Some(status),
                recovery_wrong_metadata: mismatch,
                ..Default::default()
            },
            Default::default(),
            4,
        )
        .await;
        assert!(matches!(
            fixture.runner.launch(fixture.custom_recovery_request("interrupted")).await,
            LaunchOutcome::Accepted { response_id } if response_id == "resp_fake"
        ));
        let outcomes = fixture.terminal().await;
        if mismatch {
            assert!(matches!(
                outcomes.last(),
                Some(TurnOutcome::Failed { code, .. }) if code == "stream_lost"
            ));
        } else {
            assert!(matches!(
                outcomes.last(),
                Some(TurnOutcome::Completed { final_text: Some(text) }) if text == "recovered answer"
            ));
        }
        assert!(matches!(
            outcomes.as_slice(),
            [
                TurnOutcome::Accepted { .. },
                TurnOutcome::TextDelta { .. },
                _
            ]
        ));
        assert_eq!(fixture.fake.requests().len(), 1);
        let all_calls = fixture.fake.log();
        let paths: Vec<_> = all_calls
            .iter()
            .filter(|request| request.method == "GET")
            .map(|request| request.path.as_str())
            .collect();
        let expected = if status == "in_progress" {
            &[
                "/api/v2/responses/resp_fake",
                "/api/v2/responses/resp_fake/events",
            ][..]
        } else {
            &["/api/v2/responses/resp_fake"][..]
        };
        assert_eq!(paths, expected);
        fixture.fake.close().await;
    }
}

#[tokio::test]
async fn delayed_acceptance_defers_but_keeps_consuming() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            response_delay: Duration::from_millis(250),
            ..Default::default()
        },
        TurnConfig {
            accept_budget: Duration::from_millis(40),
            ..Default::default()
        },
        4,
    )
    .await;
    let started = Instant::now();
    assert!(matches!(
        fixture.runner.launch(fixture.request("slow")).await,
        LaunchOutcome::Deferred
    ));
    assert!(started.elapsed() < Duration::from_millis(150));
    assert_eq!(fixture.tracker.len(), 1);
    let outcomes = fixture.terminal().await;
    assert!(matches!(
        outcomes.first(),
        Some(TurnOutcome::Accepted { .. })
    ));
    assert!(matches!(
        outcomes.last(),
        Some(TurnOutcome::Completed { .. })
    ));
    assert!(started.elapsed() >= Duration::from_millis(250));
    fixture.fake.close().await;
}
#[tokio::test]
async fn acceptance_budget_includes_time_before_launch_and_waiting_for_headers() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            header_delay: Duration::from_millis(200),
            ..Default::default()
        },
        TurnConfig {
            accept_budget: Duration::from_millis(100),
            ..Default::default()
        },
        4,
    )
    .await;
    let mut request = fixture.request("headers");
    request.received_at = Instant::now() - Duration::from_millis(80);
    let started = Instant::now();
    assert!(matches!(
        fixture.runner.launch(request).await,
        LaunchOutcome::Deferred
    ));
    assert!(started.elapsed() < Duration::from_millis(80));
    assert!(matches!(
        fixture.terminal().await.last(),
        Some(TurnOutcome::Completed { .. })
    ));
    fixture.fake.close().await;
}
#[tokio::test]
async fn upstream_busy_retries_same_key_and_missing_conversation_is_created_once() {
    for config in [
        FakeProviderConfig {
            busy_once: true,
            ..Default::default()
        },
        FakeProviderConfig {
            missing_turn_once: true,
            ..Default::default()
        },
        FakeProviderConfig {
            missing_turn_once: true,
            busy_once: true,
            ..Default::default()
        },
    ] {
        let missing = config.missing_turn_once;
        let busy = config.busy_once;
        let mut fixture = Fixture::new(config, Default::default(), 4).await;
        assert!(matches!(
            fixture.runner.launch(fixture.request("retry")).await,
            LaunchOutcome::Accepted { .. }
        ));
        fixture.terminal().await;
        let requests = fixture.fake.requests();
        let posts: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
        assert_eq!(posts.len(), 1 + usize::from(missing) + usize::from(busy));
        for pair in posts.windows(2) {
            assert_eq!(pair[0].idempotency_key, pair[1].idempotency_key);
            assert_eq!(pair[0].body, pair[1].body);
        }
        assert_eq!(
            requests.iter().filter(|r| r.method == "PUT").count(),
            usize::from(missing)
        );
        fixture.fake.close().await;
    }
}
#[tokio::test]
async fn readiness_retry_honours_retry_after_after_ingress_returns_deferred() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            busy_once: true,
            busy_retry_after: Some(1),
            ..Default::default()
        },
        TurnConfig {
            accept_budget: Duration::from_millis(100),
            busy_retry_max: Duration::from_secs(5),
            ..Default::default()
        },
        4,
    )
    .await;
    assert!(matches!(
        fixture
            .runner
            .launch(fixture.request("cold-readiness"))
            .await,
        LaunchOutcome::Deferred
    ));
    assert!(matches!(
        fixture.terminal().await.last(),
        Some(TurnOutcome::Completed { .. })
    ));
    let requests = fixture.fake.requests();
    let posts: Vec<_> = requests.iter().filter(|r| r.method == "POST").collect();
    assert_eq!(posts.len(), 2);
    assert!(posts[1].received_at.duration_since(posts[0].received_at) >= Duration::from_secs(1));
    assert_eq!(posts[0].idempotency_key, posts[1].idempotency_key);
    assert_eq!(posts[0].body, posts[1].body);
    fixture.fake.close().await;
}

#[tokio::test]
async fn permanent_busy_and_local_capacity_refuse_with_provider_busy() {
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            busy_always: true,
            ..Default::default()
        },
        TurnConfig {
            busy_retry_max: Duration::from_millis(30),
            ..Default::default()
        },
        1,
    )
    .await;
    let result = fixture.runner.launch(fixture.request("busy")).await;
    assert!(
        matches!(result,LaunchOutcome::Refused(problem) if problem.code==cmaeg_core::problem::ProblemCode::ProviderBusy)
    );
    fixture.terminal().await;
    fixture.fake.close().await;
    let mut fixture = Fixture::new(
        FakeProviderConfig {
            terminal_delay: Duration::from_millis(200),
            ..Default::default()
        },
        Default::default(),
        1,
    )
    .await;
    assert!(matches!(
        fixture.runner.launch(fixture.request("one")).await,
        LaunchOutcome::Accepted { .. }
    ));
    let started = Instant::now();
    assert!(
        matches!(fixture.runner.launch(fixture.request("two")).await,LaunchOutcome::Refused(problem) if problem.code==cmaeg_core::problem::ProblemCode::ProviderBusy)
    );
    assert!(started.elapsed() < Duration::from_millis(50));
    assert_eq!(fixture.fake.requests().len(), 1);
    fixture.terminal().await;
    fixture.terminal().await;
    fixture.fake.close().await;
}
#[tokio::test]
async fn broken_streams_and_total_timeout_emit_named_failures() {
    for (config, turn_timeout, reason, accepted) in [
        (
            FakeProviderConfig {
                disconnect_after: Some(0),
                ..Default::default()
            },
            Duration::from_secs(1),
            "stream_lost",
            false,
        ),
        (
            FakeProviderConfig {
                disconnect_after: Some(2),
                ..Default::default()
            },
            Duration::from_secs(1),
            "stream_lost",
            true,
        ),
        (
            FakeProviderConfig {
                terminal_delay: Duration::from_secs(1),
                ..Default::default()
            },
            Duration::from_millis(50),
            "turn_timeout",
            true,
        ),
    ] {
        let mut fixture = Fixture::new(
            config,
            TurnConfig {
                turn_timeout,
                ..Default::default()
            },
            1,
        )
        .await;
        let outcome = fixture.runner.launch(fixture.request("failure")).await;
        assert_eq!(matches!(outcome, LaunchOutcome::Accepted { .. }), accepted);
        let outcomes = fixture.terminal().await;
        assert!(
            matches!(outcomes.last(),Some(TurnOutcome::Failed {code,..}) if code==reason),
            "{outcomes:?}"
        );
        fixture.fake.close().await;
    }
}
#[tokio::test]
async fn the_default_consumer_named_explicitly_serves_the_turn() {
    let mut fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let mut request = fixture.request("explicit-default");
    request.consumer = Some(ConsumerId::default_consumer());
    assert!(matches!(
        fixture.runner.launch(request).await,
        LaunchOutcome::Accepted { .. }
    ));
    let outcomes = fixture.terminal().await;
    assert!(matches!(
        outcomes.last(),
        Some(TurnOutcome::Completed { .. })
    ));
    let requests = fixture.fake.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer runtime-test-provider-bearer")
    );
    fixture.fake.close().await;
}
#[tokio::test]
async fn a_turn_for_an_unconfigured_consumer_fails_before_any_provider_call() {
    let mut fixture = Fixture::new(Default::default(), Default::default(), 4).await;
    let mut request = fixture.request("unserved");
    request.consumer = Some(ConsumerId::parse("csm_0123456789ab").unwrap());
    let LaunchOutcome::Refused(problem) = fixture.runner.launch(request).await else {
        panic!("a turn without a serving consumer must be refused");
    };
    assert_eq!(problem.status, 503);
    assert_eq!(problem.code, ProblemCode::ProviderUnavailable);
    assert_eq!(problem.detail.as_deref(), Some("provider_unavailable"));
    let outcomes = fixture.terminal().await;
    assert!(
        matches!(outcomes.as_slice(), [TurnOutcome::Failed { code, .. }] if code == "provider_unavailable"),
        "{outcomes:?}"
    );
    assert!(fixture.fake.requests().is_empty());
    fixture.fake.close().await;
}
