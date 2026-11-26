use super::*;

#[tokio::test]
async fn empty_ledger_lookup_failure_remains_fenced_without_reserving_a_command() {
    let test = fixture().await;
    let body = Some(json!({"text":"one"}));
    test.authority
        .custody_unavailable
        .store(true, Ordering::SeqCst);
    let first =
        agents_tests::response(&test.router, request("admin", "input", "one", body.clone())).await;
    assert_eq!(first.0, StatusCode::SERVICE_UNAVAILABLE);
    let problem: Value = serde_json::from_slice(&first.1).unwrap();
    assert_eq!(problem["code"], "instance_service_unavailable");
    assert!(records(&test).await.is_empty());
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 0);

    test.authority
        .custody_unavailable
        .store(false, Ordering::SeqCst);
    for expired in [false, true] {
        test.outer.recovery.store(expired, Ordering::SeqCst);
        let (status, problem) = agents_tests::json_response(
            &test.router,
            request("admin", "input", "one", body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            problem["code"],
            if expired {
                "idempotency_recovery_required"
            } else {
                "idempotency_in_flight"
            }
        );
        assert!(records(&test).await.is_empty());
        assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 0);
    }

    // Only this fixture proves the ledger is empty; the failed request cannot.
    for _ in 0..2 {
        assert_eq!(
            agents_tests::json_response(
                &test.router,
                request("admin", "input", "independent-command", body.clone())
            )
            .await,
            (StatusCode::ACCEPTED, json!({"queued":true}))
        );
    }
    assert_eq!(records(&test).await.len(), 1);
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn lookup_failure_does_not_complete_an_older_ambiguous_outer_claim() {
    for expired in [false, true] {
        let test = fixture().await;
        let body = json!({"text":"old input"});
        test.outer
            .reserve(&outer_claim("old", &body))
            .await
            .unwrap();
        test.outer.recovery.store(expired, Ordering::SeqCst);
        test.authority
            .custody_unavailable
            .store(true, Ordering::SeqCst);
        assert_eq!(
            agents_tests::response(
                &test.router,
                request("admin", "input", "old", Some(body.clone()))
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );

        test.authority
            .custody_unavailable
            .store(false, Ordering::SeqCst);
        let (status, problem) =
            agents_tests::json_response(&test.router, request("admin", "input", "old", Some(body)))
                .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            problem["code"],
            if expired {
                "idempotency_recovery_required"
            } else {
                "idempotency_in_flight"
            }
        );
        assert!(records(&test).await.is_empty());
        assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn lookup_failure_preserves_recovery_of_an_ambiguous_committed_command() {
    let test = fixture().await;
    let body = Some(json!({"text":"one"}));
    test.commands.unknown.store(true, Ordering::SeqCst);
    assert_eq!(
        agents_tests::response(&test.router, request("admin", "input", "one", body.clone()))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let original = records(&test).await;
    assert_eq!(original.len(), 1);
    test.authority
        .custody_unavailable
        .store(true, Ordering::SeqCst);
    assert_eq!(
        agents_tests::response(&test.router, request("admin", "input", "one", body.clone()))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    test.authority
        .custody_unavailable
        .store(false, Ordering::SeqCst);
    assert_eq!(
        agents_tests::json_response(&test.router, request("admin", "input", "one", body)).await,
        (StatusCode::ACCEPTED, json!({"queued":true}))
    );
    assert_eq!(records(&test).await, original);
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn initial_body_validation_refusal_still_replays_its_exact_receipt() {
    let test = fixture().await;
    let body = Some(json!({"unexpected":"field"}));
    let first = agents_tests::response(
        &test.router,
        request("admin", "input", "invalid-body", body.clone()),
    )
    .await;
    assert_eq!(first.0, StatusCode::UNPROCESSABLE_ENTITY);
    test.authority
        .custody_unavailable
        .store(true, Ordering::SeqCst);
    for expired in [false, true] {
        test.outer.recovery.store(expired, Ordering::SeqCst);
        assert_eq!(
            agents_tests::response(
                &test.router,
                request("admin", "input", "invalid-body", body.clone()),
            )
            .await,
            first
        );
    }
    assert!(records(&test).await.is_empty());
    assert_eq!(test.commands.reserves.load(Ordering::SeqCst), 0);
}
