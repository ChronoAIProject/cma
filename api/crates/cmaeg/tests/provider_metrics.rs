//! A dedicated process keeps the global recorder isolated while stream tasks run.
use axum::{body::Body, routing::post, Router};
use cmaeg::providers::{ProviderClient, ProviderError, ProviderRegistry, TurnRequest};
use cmaeg_core::{channel::Secret, ids::IdempotencyKey};
use futures_util::{stream, StreamExt};
use metrics_exporter_prometheus::PrometheusHandle;
use std::{collections::BTreeMap, convert::Infallible, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Semaphore};

fn sample(handle: &PrometheusHandle, name: &str, outcome: Option<&str>) -> f64 {
    let text = handle.render();
    let values: Vec<f64> = text
        .lines()
        .filter_map(|line| {
            if !line.starts_with(&format!("{name}{{"))
                || !line.contains("provider=\"metrics_fixture\"")
                || outcome.is_some_and(|outcome| !line.contains(&format!("outcome=\"{outcome}\"")))
            {
                return None;
            }
            line.split_whitespace().last()?.parse().ok()
        })
        .collect();
    assert_eq!(values.len(), 1, "missing or duplicate {name}: {text}");
    values[0]
}
async fn released(handle: &PrometheusHandle) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while sample(handle, "cmaeg_provider_permits_in_use", None) != 0. {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("stream task retained its permit after completion/disconnect");
    assert_eq!(sample(handle, "cmaeg_provider_permits_available", None), 1.);
}

#[tokio::test]
async fn live_stream_metrics_follow_saturation_cancellation_disconnect_and_completion() {
    let handle = cmaeg::telemetry::install().unwrap();
    let finish = Arc::new(Semaphore::new(0));
    let notify = finish.clone();
    let app = Router::new().route("/responses", post(move || {
        let notify = notify.clone();
        async move {
            let created = "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_fixture\"}}\n\n";
            let completed = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fixture\"}}\n\n";
            let frames = stream::once(async move { Ok::<_, Infallible>(created) }).chain(
                stream::once(async move { notify.acquire().await.unwrap().forget(); Ok::<_, Infallible>(completed) }),
            );
            ([("content-type", "text/event-stream")], Body::from_stream(frames))
        }
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let registry = ProviderRegistry::parse(
        &serde_json::json!([{
            "slug":"metrics_fixture", "base_url":format!("http://{address}"),
            "kind":"responses_http", "max_inflight":2,
            "paths":{"agent_card":"/card", "binding":"/bindings/{binding_id}",
                "conversation":"/bindings/{binding_id}/conversations/{conversation_id}",
                "responses":"/responses"}
        }])
        .to_string(),
    )
    .unwrap();
    let client =
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(2), 1).unwrap();
    let provider = "metrics_fixture".parse().unwrap();
    let request = |event: &str| TurnRequest {
        timing: None,
        conversation_id: "conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".parse().unwrap(),
        input: vec![],
        idempotency_key: IdempotencyKey::from_event(&event.parse().unwrap()),
        metadata: BTreeMap::new(),
        agent_key: Secret::new("fixture-credential-value"),
        event_context: None,
    };
    assert_eq!(sample(&handle, "cmaeg_provider_permit_capacity", None), 1.);
    let mut first = client
        .start_turn_immediate(&provider, request("first"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut first.accepted)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sample(&handle, "cmaeg_provider_permits_in_use", None), 1.);
    assert_eq!(
        sample(&handle, "cmaeg_provider_permits_available", None),
        0.
    );
    assert!(matches!(
        client
            .start_turn_immediate(&provider, request("full"))
            .await,
        Err(ProviderError::Capacity)
    ));
    assert!(matches!(
        client.start_turn(&provider, request("timeout")).await,
        Err(ProviderError::Busy { .. })
    ));
    {
        let waiting = client.start_turn(&provider, request("cancelled"));
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        assert_eq!(sample(&handle, "cmaeg_provider_permit_waiters", None), 1.);
        assert!(
            sample(
                &handle,
                "cmaeg_provider_permit_oldest_wait_timestamp_seconds",
                None
            ) > 0.
        );
    }
    assert_eq!(sample(&handle, "cmaeg_provider_permit_waiters", None), 0.);
    assert_eq!(
        sample(
            &handle,
            "cmaeg_provider_permit_oldest_wait_timestamp_seconds",
            None
        ),
        0.
    );
    // The upstream is still idle: consumer disconnect must release the permit.
    drop(first);
    released(&handle).await;
    assert_eq!(
        sample(&handle, "cmaeg_provider_permit_held_seconds_count", None),
        1.
    );
    let mut second = client
        .start_turn_immediate(&provider, request("completed"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), &mut second.accepted)
        .await
        .unwrap()
        .unwrap();
    // Wake both HTTP fixtures; the disconnected request has no local ownership.
    finish.add_permits(2);
    tokio::time::timeout(Duration::from_secs(2), async {
        while second.events.recv().await.is_some() {}
    })
    .await
    .unwrap();
    released(&handle).await;
    assert_eq!(
        sample(&handle, "cmaeg_provider_permit_held_seconds_count", None),
        2.
    );
    for (outcome, count) in [
        ("acquired", 2.),
        ("full", 1.),
        ("timeout", 1.),
        ("cancelled", 1.),
    ] {
        assert_eq!(
            sample(
                &handle,
                "cmaeg_provider_permit_acquisitions_total",
                Some(outcome)
            ),
            count
        );
    }
    let text = handle.render();
    for private in [
        "conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "fixture-credential-value",
        "resp_fixture",
    ] {
        assert!(
            !text.contains(private),
            "private value became a metric label"
        );
    }
    server.abort();
    let _ = server.await;
}
