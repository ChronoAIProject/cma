use super::*;

fn control(text: &str) -> HttpFixture {
    let mut fixture = HttpFixture::new(false);
    fixture.protocol.body["content"]["text"] = json!(text);
    fixture.protocol.raw_mut()["message"]["text"] = json!(text);
    fixture
}

#[tokio::test]
async fn reserved_control_http_waits_for_durable_refusal_and_creates_no_message() {
    for (text, code) in [
        (
            "CMA-CONTROL/1 cancel ccr_source",
            "channel_controls_disabled",
        ),
        (
            "CMA-CONTROL/1 steer ccr_source\nCheck the migration first.",
            "channel_controls_disabled",
        ),
        (
            "CMA-CONTROL/2 cancel ccr_source",
            "channel_control_version_unsupported",
        ),
        (
            "CMA-CONTROL/1 approve ccr_source",
            "approval_dashboard_only",
        ),
        ("CMA-CONTROL/1 cancel", "invalid_channel_control_request"),
    ] {
        let fixture = control(text);
        let app = fixture.router();
        let request = fixture.request();
        let pending = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        fixture.provider.entered.notified().await;
        assert!(!pending.is_finished());
        assert!(fixture.provider.refusals.lock().await.is_empty());
        fixture.provider.commits.add_permits(1);
        let response = pending.await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let original = value(response).await;
        assert_eq!(original["status"], "control_refused");
        assert_eq!(original["receipt"]["code"], code);
        assert!(original["receipt"].get("command_id").is_none());
        assert!(original["receipt"].get("inbox_id").is_none());
        assert!(fixture.provider.rows.lock().await.is_empty());
        fixture.provider.commits.add_permits(1);
        let replay = value(fixture.router().oneshot(fixture.request()).await.unwrap()).await;
        assert_eq!(replay, original);
        assert_eq!(fixture.provider.refusals.lock().await.len(), 1);
    }
}

#[tokio::test]
async fn reserved_control_authentication_identity_and_unknown_commit_keep_existing_errors() {
    let mut fixture = control("CMA-CONTROL/1 cancel ccr_source");
    let mut request = fixture.request();
    request
        .headers_mut()
        .insert("x-nyxid-callback-token", "invalid".parse().unwrap());
    let response = fixture.router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(fixture.provider.inbox_calls.load(Ordering::SeqCst), 0);

    fixture.protocol.raw_mut()["message"]["text"] = json!("different raw text");
    fixture.provider.commits.add_permits(1);
    let result = value(fixture.router().oneshot(fixture.request()).await.unwrap()).await;
    assert_eq!(result["status"], "inbox_persisted");
    assert_ne!(result["receipt"]["admission"], "accepted");
    assert!(fixture.provider.refusals.lock().await.is_empty());

    let fixture = control("CMA-CONTROL/1 cancel ccr_other");
    fixture.provider.failure.store(2, Ordering::SeqCst);
    fixture.provider.commits.add_permits(1);
    let response = fixture.router().oneshot(fixture.request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(value(response).await["acceptance"], "unknown");
    fixture.provider.failure.store(0, Ordering::SeqCst);
    fixture.provider.commits.add_permits(1);
    let result = value(fixture.router().oneshot(fixture.request()).await.unwrap()).await;
    assert_eq!(result["status"], "control_refused");
    assert!(fixture.provider.rows.lock().await.is_empty());
}

#[tokio::test]
async fn exact_legacy_message_prefix_keeps_original_meaning() {
    let fixture = control("CMA-CONTROL/1 cancel ccr_legacy");
    let old = fixture.protocol.write().unwrap().record().clone();
    fixture
        .provider
        .rows
        .lock()
        .await
        .insert(old.id.clone(), old.clone());
    fixture.provider.commits.add_permits(1);
    let result = value(fixture.router().oneshot(fixture.request()).await.unwrap()).await;
    assert_eq!(result["status"], "inbox_persisted");
    assert_eq!(result["receipt"]["inbox_id"], old.id);
    assert_eq!(result["receipt"]["admission"], "accepted");
    assert!(fixture.provider.refusals.lock().await.is_empty());
}
