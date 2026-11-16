use super::*;
use fixture::Reply;
use std::time::Duration;

fn success() -> Reply {
    Reply::json(
        StatusCode::OK,
        json!({"orgs":[{"id":"org-a","your_role":"viewer"}]}),
    )
}

fn failure(status: u16) -> Reply {
    Reply::json(
        StatusCode::from_u16(status).unwrap(),
        json!({"detail":"must-not-leak-retry-body"}),
    )
}

#[tokio::test]
async fn transient_inventory_failure_retries_once_then_returns_current_choices() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        for status in [429, 502, 503, 504] {
            let mut transient = failure(status);
            transient.retry_after = Some("0");
            *f.control.sequence.write().await = [transient, success()].into();
            let before = f.calls();
            let (status, body, events) = f.boundaries(manager, false).await;
            assert_eq!(status, StatusCode::OK, "transient retry did not recover");
            assert_eq!(body["items"][0]["id"], "org-a");
            assert_eq!(body["items"][0]["role"], "viewer");
            assert_eq!(f.calls() - before, 2);
            assert_eq!(
                events.len(),
                1,
                "recovered attempt is not a final request failure"
            );
            assert_eq!(
                events[0]["fields"]["event"],
                "organization_authority_failure"
            );
        }
    }
    f.close().await;
}

#[tokio::test]
async fn repeated_transient_failure_stops_after_two_attempts_with_the_last_status() {
    let f = Fixture::new().await;
    *f.control.sequence.write().await = [failure(502), failure(504), success()].into();
    let (status, body, events) = f.boundaries(0, false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "organization_authority_unavailable");
    assert_eq!(f.calls(), 2);
    assert_eq!(
        f.control.sequence.read().await.len(),
        1,
        "third attempt forbidden"
    );
    assert_eq!(events.len(), 3, "two attempts and one final HTTP boundary");
    assert_eq!(events[0]["fields"]["status"], 502);
    assert_eq!(events[1]["fields"]["status"], 504);
    assert!(body["detail"]
        .as_str()
        .unwrap()
        .contains("upstream_status=504"));
    f.close().await;
}

#[tokio::test]
async fn definite_denial_is_never_retried_or_hidden_by_an_earlier_outage() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        for denial in [401, 403, 404] {
            for preceded_by_outage in [false, true] {
                let mut sequence = Vec::new();
                if preceded_by_outage {
                    sequence.push(failure(503));
                }
                sequence.extend([failure(denial), success()]);
                *f.control.sequence.write().await = sequence.into();
                let before = f.calls();
                let (status, body, events) = f.boundaries(manager, false).await;
                assert_eq!(status, StatusCode::FORBIDDEN);
                assert_eq!(body["code"], "resource_forbidden");
                assert_eq!(f.calls() - before, if preceded_by_outage { 2 } else { 1 });
                assert_eq!(f.control.sequence.read().await.len(), 1);
                assert!(events
                    .iter()
                    .all(|event| event["fields"]["event"] != "organization_choices_failure"));
                assert_eq!(
                    events.last().unwrap()["fields"]["disposition"],
                    "not_member"
                );
            }
        }
    }
    f.close().await;
}

#[tokio::test]
async fn retry_after_that_exceeds_the_lookup_budget_is_not_shortened_or_slept() {
    let f = Fixture::new().await;
    for manager in 0..2 {
        let mut transient = failure(429);
        transient.retry_after = Some("60");
        *f.control.sequence.write().await = [transient, success()].into();
        let before = f.calls();
        let started = tokio::time::Instant::now();
        let (status, body) = f.request(manager).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "organization_authority_unavailable");
        assert_eq!(f.calls() - before, 1);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "provider delay exceeds available budget"
        );
        assert_eq!(f.control.sequence.read().await.len(), 1);
    }
    f.close().await;
}

#[tokio::test]
async fn short_retry_after_is_respected_before_the_second_attempt() {
    let f = Fixture::new().await;
    let mut transient = failure(503);
    transient.retry_after = Some("1");
    *f.control.sequence.write().await = [transient, success()].into();
    let started = tokio::time::Instant::now();
    assert_eq!(f.request(0).await.0, StatusCode::OK);
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(f.calls(), 2);
    f.close().await;
}

#[tokio::test]
async fn concurrent_managers_retry_their_own_request_without_sharing_completed_authority() {
    let f = Fixture::new().await;
    *f.control.sequence.write().await = [failure(503), failure(503), success(), success()].into();
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    *f.control.inventory_gate.write().await = Some(gate.clone());
    let (first, second, ()) = tokio::join!(f.request(0), f.request(1), async {
        tokio::time::timeout(
            Duration::from_secs(3),
            f.control.inventory_arrivals.acquire_many(2),
        )
        .await
        .unwrap()
        .unwrap()
        .forget();
        // Both managers reached the provider before either retry can start.
        f.control.inventory_gate.write().await.take();
        gate.add_permits(2);
    });
    assert_eq!(first.0, StatusCode::OK);
    assert_eq!(second.0, StatusCode::OK);
    assert_eq!(f.calls(), 4);
    f.reply(StatusCode::FORBIDDEN, json!({})).await;
    let (first, second) = tokio::join!(f.request(0), f.request(1));
    assert_eq!(first.0, StatusCode::FORBIDDEN);
    assert_eq!(second.0, StatusCode::FORBIDDEN);
    assert_eq!(f.calls(), 6);
    f.close().await;
}

#[tokio::test]
async fn unknown_status_and_invalid_evidence_do_not_consume_a_successor_reply() {
    let f = Fixture::new().await;
    let malformed = Reply::json(
        StatusCode::OK,
        json!({"orgs":[{"id":"org-a","your_role":"unknown"}]}),
    );
    for reply in [failure(500), failure(501), malformed] {
        *f.control.sequence.write().await = [reply, success()].into();
        let before = f.calls();
        assert_eq!(f.request(0).await.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(f.calls() - before, 1);
        assert_eq!(f.control.sequence.read().await.len(), 1);
    }
    f.close().await;
}
