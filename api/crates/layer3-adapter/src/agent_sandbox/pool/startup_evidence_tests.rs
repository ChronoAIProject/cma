use super::*;

const PODS: &str = "/api/v1/namespaces/cma-test/pods";

fn failed_read(bound: &Bound, path: &str, status: u16) {
    bound.fixture.state.lock().unwrap().read_replies.push_back((
        path.into(),
        status,
        json!({"apiVersion":"v1","kind":"Status","status":"Failure",
            "reason":"FixtureFailure","message":"optional read failed","code":status}),
    ));
}

async fn skipped_evidence_preserves_count(change: &str) {
    let bound = fixture(true).await;
    restart(&bound, 1, 0, 2, "Error");
    adopt(&bound).await.unwrap().unwrap(); // One counted failure.
    restart(&bound, 2, 0, 2, "Error");
    if change == "error" {
        // A non-retryable HTTP refusal reaches the optional-read boundary;
        // kube-rs transparently retries a one-shot 503 before returning.
        failed_read(&bound, POD, 403);
    } else {
        let mut state = bound.fixture.state.lock().unwrap();
        let mut evidence = state.rows[POD].clone();
        match change {
            "uid" => evidence["metadata"]["uid"] = json!("replacement-pod"),
            "name" => evidence["metadata"]["name"] = json!("replacement-name"),
            "owner" => evidence["metadata"]["ownerReferences"][0]["uid"] = json!("foreign-owner"),
            _ => unreachable!(),
        }
        // Only the optional GET sees this conflicting snapshot. Subsequent
        // ordinary assignment proofs retain their independent authority checks.
        state.read_replies.push_back((POD.into(), 200, evidence));
    }
    adopt(&bound).await.unwrap().unwrap();
    assert_eq!(deliveries(&bound), 3);
    assert!(bound.fixture.state.lock().unwrap().read_replies.is_empty());
    restart(&bound, 3, 0, 2, "Error");
    adopt(&bound).await.unwrap().unwrap(); // Exactly two counted failures.
    restart(&bound, 4, 0, 2, "Error");
    assert_eq!(hold(adopt(&bound).await).consecutive, 3);
    assert_eq!(
        deliveries(&bound),
        4,
        "failed evidence neither increments nor resets"
    );
}

#[tokio::test]
async fn startup_breaker_mismatched_evidence_identity_cannot_reject_delivery_or_change_count() {
    for change in ["uid", "name", "owner"] {
        skipped_evidence_preserves_count(change).await;
    }
}

#[tokio::test]
async fn startup_breaker_failed_evidence_get_cannot_reject_delivery_or_change_count() {
    skipped_evidence_preserves_count("error").await;
}

#[tokio::test]
async fn startup_breaker_open_inventory_errors_fall_through_to_ordinary_readiness_and_hold() {
    for status in [403, 404] {
        let bound = fixture(true).await;
        open(&bound).await;
        bound.fixture.state.lock().unwrap().calls.clear();
        failed_read(&bound, PODS, status);
        assert_eq!(hold(adopt(&bound).await).consecutive, 3);
        let state = bound.fixture.state.lock().unwrap();
        assert!(state.read_replies.is_empty());
        assert_eq!(
            state
                .calls
                .iter()
                .filter(|(call, _)| call == &format!("GET {PODS}"))
                .count(),
            2,
            "failed open pre-check must fall through to wait_ready"
        );
        assert_eq!(state.assignments.len(), 3);
    }
}

#[tokio::test]
async fn startup_breaker_open_evidence_errors_fall_through_without_resetting_hold() {
    let bound = fixture(true).await;
    open(&bound).await;
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(15 * 60)).await;
    io(adopt(&bound)).await.unwrap().unwrap();
    restart(&bound, 4, 0, 2, "Error");
    bound.fixture.state.lock().unwrap().calls.clear();
    failed_read(&bound, POD, 403); // Open pre-check.
    failed_read(&bound, POD, 403); // Ordinary assign_and_bind.
    assert_eq!(hold(io(adopt(&bound)).await).consecutive, 3);
    let state = bound.fixture.state.lock().unwrap();
    assert!(state.read_replies.is_empty());
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|(call, _)| call == &format!("GET {PODS}"))
            .count(),
        2
    );
    assert_eq!(state.assignments.len(), 4);
}
