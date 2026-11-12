//! Real Custom HTTP intake and provider sockets; no production identities.
use super::{fake_provider::FakeProviderConfig, support::App};
use serde_json::Value;
use std::time::{Duration, Instant};

fn enabled() -> cmaeg::TurnConfig {
    cmaeg::TurnConfig {
        context_retry_enabled: true,
        ..Default::default()
    }
}
fn reads(app: &App) -> Vec<super::fake_provider::ReceivedRequest> {
    app.provider
        .log()
        .into_iter()
        .filter(|r| r.method == "GET")
        .collect()
}

#[tokio::test]
async fn disabled_flag_preserves_one_read_and_original_refusal() {
    let mut app = App::new(
        |_| {},
        FakeProviderConfig {
            context_status: Some(503),
            ..Default::default()
        },
        Default::default(),
        0,
    )
    .await;
    let response = app
        .request(&app.url, app.event("disabled-retry").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert!(response.headers().get("retry-after").is_none());
    app.drain().await;
    assert_eq!(reads(&app).len(), 1);
    assert!(app.provider.requests().is_empty());
    assert!(app.receiver.events().is_empty());
}

#[tokio::test]
async fn slow_context_then_absence_retries_same_read_before_one_admission() {
    let mut app = App::new(
        |_| {},
        FakeProviderConfig {
            exact_admission: true,
            context_steps: vec![(Duration::from_millis(3200), None, None)],
            ..Default::default()
        },
        enabled(),
        0,
    )
    .await;
    let started = Instant::now();
    let response = app
        .request(&app.url, app.event("slow-context").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    app.drain().await;
    let lookups = reads(&app);
    assert_eq!(lookups.len(), 2);
    assert_eq!(lookups[0].path, lookups[1].path);
    assert_eq!(lookups[0].authorization, lookups[1].authorization);
    assert!(
        lookups[1]
            .received_at
            .duration_since(lookups[0].received_at)
            >= Duration::from_millis(3500)
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    let writes = app.provider.requests();
    assert_eq!(writes.len(), 1);
    assert!(writes[0].received_at > lookups[1].received_at);
    assert_eq!(app.provider.admitted().len(), 1);
    assert_eq!(app.receiver.events().len(), 1);
}

#[tokio::test]
async fn retrying_retained_context_preserves_output_and_content_conflicts() {
    for changed in [false, true] {
        let mut app = App::new(
            |_| {},
            FakeProviderConfig {
                exact_admission: true,
                context_steps: vec![
                    (Duration::ZERO, None, None),
                    (Duration::ZERO, Some(503), None),
                ],
                ..Default::default()
            },
            enabled(),
            0,
        )
        .await;
        let event = app.event("retained-retry");
        let first = app
            .request(&format!("{}?mode=stream", app.url), event.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        first.text().await.unwrap();
        let original = app.provider.admitted();
        let mut retry = event.clone();
        if changed {
            retry["data"]["a"] = 99.into();
        }
        let response = app
            .request(&app.url, retry.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), if changed { 409 } else { 202 });
        app.drain().await;
        assert_eq!(reads(&app).len(), 3);
        assert_eq!(app.provider.requests().len(), 1);
        assert_eq!(app.provider.admitted(), original);
        assert_eq!(app.receiver.events().len(), 1);
    }
}

#[tokio::test]
async fn explicit_auth_refusal_and_malformed_context_never_retry_or_submit() {
    for (upstream, expected) in [(401, 401), (403, 403), (409, 503), (200, 503)] {
        let mut app = App::new(
            |_| {},
            FakeProviderConfig {
                context_status: Some(upstream),
                ..Default::default()
            },
            enabled(),
            0,
        )
        .await;
        let response = app
            .request(&app.url, app.event("context-refusal").to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
        assert!(response.headers().get("retry-after").is_none());
        app.drain().await;
        assert_eq!(reads(&app).len(), 1);
        assert!(app.provider.requests().is_empty());
        assert!(app.receiver.events().is_empty());
    }
}

#[tokio::test]
async fn transient_exhaustion_refuses_honestly_with_retry_after_and_no_callback() {
    let mut app = App::new(
        |_| {},
        FakeProviderConfig {
            context_status: Some(503),
            ..Default::default()
        },
        cmaeg::TurnConfig {
            accept_budget: Duration::from_millis(1),
            ..enabled()
        },
        0,
    )
    .await;
    let response = app
        .request(
            &format!("{}?mode=stream", app.url),
            app.event("retry-exhausted").to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "1");
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["detail"], "provider_context_unavailable");
    assert!(problem.get("retry_after_seconds").is_none());
    app.drain().await;
    assert_eq!(reads(&app).len(), 2);
    assert!(app.provider.requests().is_empty());
    assert!(app.receiver.events().is_empty());
    assert!(app.waiters.is_empty());
}

#[tokio::test]
async fn upstream_retry_after_exceeding_budget_is_forwarded_without_an_early_probe() {
    let mut app = App::new(
        |_| {},
        FakeProviderConfig {
            context_steps: vec![(Duration::ZERO, Some(429), Some("10".into()))],
            ..Default::default()
        },
        enabled(),
        0,
    )
    .await;
    let response = app
        .request(&app.url, app.event("retry-minimum").to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], "10");
    app.drain().await;
    assert_eq!(reads(&app).len(), 1);
    assert!(app.provider.requests().is_empty());
    assert!(app.receiver.events().is_empty());
}
