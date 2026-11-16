use super::*;
use serde_json::Value;

fn event(events: &[Value], stage: &str, reason: &str, disposition: &str) {
    assert_eq!(
        events.len(),
        1,
        "one diagnostic for the failed authority boundary"
    );
    assert_eq!(events[0]["fields"]["operation"], "choices");
    assert_eq!(events[0]["fields"]["stage"], stage);
    assert_eq!(events[0]["fields"]["reason"], reason);
    assert_eq!(events[0]["fields"]["disposition"], disposition);
}

#[tokio::test]
async fn status_failure_identifies_the_org_dependency_after_successful_identity() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        for upstream in [429, 503, 401, 403, 404] {
            f.reply(
                StatusCode::from_u16(upstream).unwrap(),
                json!({"detail":"must-not-leak-upstream-body"}),
            )
            .await;
            let before = f.calls();
            let (status, _, events) = f.observed_request(manager).await;
            let denied = [401, 403, 404].contains(&upstream);
            assert_eq!(
                status,
                if denied {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                }
            );
            assert_eq!(f.calls() - before, 1);
            event(
                &events,
                "status",
                "http_status",
                if denied { "not_member" } else { "unavailable" },
            );
            assert_eq!(events[0]["fields"]["status"], upstream);
        }
    }
    f.close().await;
}

#[tokio::test]
async fn malformed_body_and_invalid_evidence_have_distinct_private_diagnostics() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        for (body, stage, reason) in [
            (
                "must-not-leak-invalid-json".to_owned(),
                "decode",
                "invalid_body",
            ),
            (
                json!({"orgs":[{"id":"../org-a","your_role":"admin"}]}).to_string(),
                "evidence",
                "invalid_evidence",
            ),
        ] {
            f.control.inventory.write().await.body = body;
            let before = f.calls();
            let (status, _, events) = f.observed_request(manager).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(f.calls() - before, 1);
            event(&events, stage, reason, "invalid_evidence");
            assert_eq!(
                events[0]["fields"]["status"],
                if stage == "decode" {
                    json!(200)
                } else {
                    Value::Null
                }
            );
        }
    }
    f.close().await;
}

#[tokio::test]
async fn principal_revalidation_failure_is_not_mislabeled_as_an_org_http_error() {
    for manager in 0..2 {
        let f = Fixture::new().await;
        f.control.revoke_on_introspection.store(2, Ordering::SeqCst);
        let (status, body, events) = f.observed_request(manager).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "organization_authority_unavailable");
        assert_eq!(f.calls(), 0);
        assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
        event(&events, "principal", "unavailable", "unavailable");
        assert_eq!(events[0]["fields"]["status"], Value::Null);
        f.close().await;
    }
}
