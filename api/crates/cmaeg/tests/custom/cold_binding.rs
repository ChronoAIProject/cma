//! The first authenticated Custom event survives a still-provisioning CMA binding.
use super::{fake_provider::FakeProviderConfig, support::App};
use cmaeg::TurnConfig;
use serde_json::Value;
use std::time::Duration;

fn cold(statuses: &[(u16, &str)]) -> FakeProviderConfig {
    FakeProviderConfig {
        missing_turn_once: true,
        exact_admission: true,
        conversation_statuses: statuses
            .iter()
            .map(|(status, code)| (*status, (*code).into()))
            .collect(),
        ..Default::default()
    }
}

#[tokio::test]
async fn cold_binding_keeps_the_first_custom_event_and_admits_it_once() {
    for deferred in [false, true] {
        let mut app = App::new(
            |_| {},
            cold(&[(409, "binding_not_ready")]),
            TurnConfig {
                accept_budget: if deferred {
                    Duration::from_millis(1)
                } else {
                    Duration::from_secs(20)
                },
                ..Default::default()
            },
            0,
        )
        .await;
        let event = app.event("cold-binding-first").to_string();
        let url = format!("{}?mode=stream", app.url);
        let response = app.request(&url, event.clone()).send().await.unwrap();
        assert_eq!(response.status(), if deferred { 202 } else { 200 });
        if deferred {
            let accepted: Value = response.json().await.unwrap();
            assert_eq!(accepted["event_id"], "cold-binding-first");
        } else {
            assert!(response
                .text()
                .await
                .unwrap()
                .contains("event: cmaeg.completed"));
        }
        app.drain().await;
        let requests = app.provider.requests();
        assert_eq!(
            requests
                .iter()
                .map(|r| r.method.as_str())
                .collect::<Vec<_>>(),
            ["POST", "PUT", "PUT", "POST"]
        );
        assert_eq!(requests[1].path, requests[2].path);
        assert_eq!(requests[1].body, requests[2].body);
        assert_eq!(requests[1].authorization, requests[2].authorization);
        assert!(
            requests[2]
                .received_at
                .duration_since(requests[1].received_at)
                >= Duration::from_millis(500)
        );
        assert_eq!(requests[0].idempotency_key, requests[3].idempotency_key);
        assert_eq!(requests[0].body, requests[3].body);
        assert_eq!(requests[0].authorization, requests[3].authorization);
        let admitted = app.provider.admitted();
        assert_eq!(admitted.len(), 1);
        assert_eq!(admitted[0].body, requests[3].body);
        assert_eq!(admitted[0].idempotency_key, requests[3].idempotency_key);
        assert_eq!(admitted[0].authorization, requests[3].authorization);
        let body: Value = serde_json::from_str(&admitted[0].body).unwrap();
        assert_eq!(
            body["event_context"]["activity"]["event_id"],
            "cold-binding-first"
        );
        assert_eq!(
            body["conversation"],
            body["event_context"]["conversation_id"]
        );
        assert!(requests[2]
            .path
            .ends_with(body["conversation"].as_str().unwrap()));
        let replay = app.request(&url, event).send().await.unwrap();
        assert_eq!(replay.status(), 202);
        assert_eq!(app.provider.admitted(), admitted);
        assert_eq!(app.provider.requests(), requests);
        assert_eq!(app.receiver.events().len(), 1);
        assert_eq!(app.receiver.events()[0]["type"], "cmaeg.response.completed");
        assert!(app.waiters.is_empty());
    }
}

#[tokio::test]
async fn cold_binding_only_retries_the_exact_cma_preparation_refusal() {
    for (provider, statuses) in [
        ("cma_codex", vec![(409, "binding_resources_unavailable")]),
        ("cma_codex", vec![(409, "binding_generation_changed")]),
        ("cma_codex", vec![(401, "binding_not_ready")]),
        ("cma_codex", vec![(403, "binding_not_ready")]),
        ("cma_codex", vec![(503, "binding_not_ready")]),
        ("nyxbot", vec![(409, "binding_not_ready")]),
        (
            "cma_codex",
            vec![(409, "binding_not_ready"), (503, "binding_unavailable")],
        ),
    ] {
        let mut app = App::new(
            |value| {
                value["provider"]["slug"] = provider.into();
                value["profile"]["metadata"]["provider"] = provider.into();
            },
            cold(&statuses),
            Default::default(),
            0,
        )
        .await;
        let response = app
            .request(&app.url, app.event("cold-binding-refused").to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 503, "{provider}: {statuses:?}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "provider_unavailable"
        );
        app.drain().await;
        let requests = app.provider.requests();
        assert_eq!(requests.iter().filter(|r| r.method == "POST").count(), 1);
        assert_eq!(
            requests.iter().filter(|r| r.method == "PUT").count(),
            statuses.len()
        );
        assert!(app.provider.admitted().is_empty());
    }
}

#[tokio::test]
async fn cold_binding_readiness_wait_is_bounded_without_submitting_a_response() {
    let mut app = App::new(
        |_| {},
        cold(&[(409, "binding_not_ready")]),
        TurnConfig {
            busy_retry_max: Duration::from_millis(30),
            ..Default::default()
        },
        0,
    )
    .await;
    let response = app
        .request(&app.url, app.event("cold-binding-budget").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "provider_busy"
    );
    app.drain().await;
    let requests = app.provider.requests();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.method.as_str())
            .collect::<Vec<_>>(),
        ["POST", "PUT"]
    );
    assert!(app.provider.admitted().is_empty());
}

#[tokio::test]
async fn cold_binding_does_not_resubmit_an_unknown_response_outcome() {
    let mut config = cold(&[(409, "binding_not_ready")]);
    config.disconnect_after = Some(0);
    let mut app = App::new(|_| {}, config, Default::default(), 0).await;
    let event = app.event("cold-binding-unknown").to_string();
    let response = app.request(&app.url, event.clone()).send().await.unwrap();
    assert_eq!(response.status(), 503);
    app.drain().await;
    let requests = app.provider.requests();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.method.as_str())
            .collect::<Vec<_>>(),
        ["POST", "PUT", "PUT", "POST"]
    );
    let admitted = app.provider.admitted();
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].body, requests[3].body);
    assert_eq!(admitted[0].idempotency_key, requests[3].idempotency_key);
    assert_eq!(admitted[0].authorization, requests[3].authorization);
    let replay = app.request(&app.url, event).send().await.unwrap();
    assert_eq!(replay.status(), 202);
    assert_eq!(app.provider.admitted(), admitted);
    assert_eq!(app.provider.requests(), requests);
}
