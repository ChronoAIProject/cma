use super::*;
use std::time::Duration;

#[tokio::test]
async fn missing_credential_reports_the_final_boundary_without_calling_org_provider() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        let (status, body, events) = f.boundaries(manager, true).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "organization_authority_unavailable");
        assert_eq!(f.calls(), 0);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["fields"]["event"], "organization_choices_failure");
        assert_eq!(events[0]["fields"]["reason"], "credential_missing");
    }
    f.close().await;
}

#[tokio::test]
async fn oversized_body_keeps_the_size_limit_and_records_both_failure_boundaries() {
    let f = Fixture::new().await;
    f.control.inventory.write().await.body = "x".repeat(1024 * 1024 + 1);
    let (status, body, events) = f.boundaries(0, false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "organization_authority_unavailable");
    assert_eq!(f.calls(), 1);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["fields"]["stage"], "body");
    assert_eq!(events[0]["fields"]["reason"], "body_too_large");
    assert_eq!(events[0]["fields"]["status"], 200);
    assert_eq!(events[1]["fields"]["event"], "organization_choices_failure");
    assert_eq!(events[1]["fields"]["reason"], "invalid_evidence");
    f.close().await;
}

#[tokio::test]
async fn success_and_denial_do_not_emit_final_unavailability() {
    let f = Fixture::new().await;
    let (status, _, events) = f.boundaries(0, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(events.is_empty());
    f.reply(StatusCode::FORBIDDEN, json!({})).await;
    let (status, _, events) = f.boundaries(1, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0]["fields"]["event"],
        "organization_authority_failure"
    );
    assert_eq!(events[0]["fields"]["disposition"], "not_member");
    f.close().await;
}

#[tokio::test]
async fn cancelled_request_does_not_claim_a_completed_authority_failure() {
    let f = Fixture::new().await;
    let events = f.cancel_held_request(0).await;
    assert!(events.is_empty());
    assert_eq!(f.calls(), 1);
    f.control
        .inventory_gate
        .write()
        .await
        .take()
        .unwrap()
        .add_permits(1);
    assert_eq!(f.request(1).await.0, StatusCode::OK);
    assert_eq!(f.calls(), 2);
    f.close().await;
}

#[tokio::test]
async fn slow_authority_preserves_the_request_budget_and_reports_final_failure() {
    let f = Fixture::new().await;
    f.control.inventory.write().await.delay = Duration::from_secs(30);
    let started = tokio::time::Instant::now();
    let (status, body, events) = f.boundaries(0, false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "organization_authority_unavailable");
    assert!(
        started.elapsed() < Duration::from_secs(17),
        "15-second authority budget expanded"
    );
    assert_eq!(f.calls(), 2, "at most one retry of a header timeout");
    // The service deadline can cancel the adapter first. The final handler
    // event remains mandatory; a cancelled adapter must not invent a result.
    let final_events: Vec<_> = events
        .iter()
        .filter(|event| event["fields"]["event"] == "organization_choices_failure")
        .collect();
    assert_eq!(final_events.len(), 1);
    assert_eq!(final_events[0]["fields"]["reason"], "unavailable");
    f.close().await;
}

#[tokio::test]
async fn timeout_after_headers_is_terminal_and_does_not_replay_the_body() {
    let f = Fixture::new().await;
    {
        let mut reply = f.control.inventory.write().await;
        reply.body = "{".into();
        reply.body_delay = Some(Duration::from_secs(30));
    }
    let started = tokio::time::Instant::now();
    let (status, body, events) = f.boundaries(0, false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "organization_authority_unavailable");
    assert_eq!(f.calls(), 1, "body reads are not replayed");
    assert!(started.elapsed() < Duration::from_secs(7));
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["fields"]["stage"], "body");
    assert_eq!(events[0]["fields"]["reason"], "timeout");
    assert_eq!(events[0]["fields"]["status"], 200);
    assert_eq!(events[1]["fields"]["event"], "organization_choices_failure");
    f.close().await;
}
