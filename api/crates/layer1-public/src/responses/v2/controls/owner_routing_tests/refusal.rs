use super::*;

#[tokio::test]
async fn owner_routing_does_not_retry_a_committed_control_after_a_lost_reply() {
    let f = fixture(true).await;
    let body = intent(&f.original, json!({"type":"cancel"})).await;
    f.route.mode.store(LOST_REPLY, Ordering::SeqCst);
    let (status, response) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("lost-reply"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{response}");
    assert_eq!(f.route.calls.lock().unwrap().len(), 1);
    let committed = f
        .store()
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items;
    assert_eq!(committed.len(), 2);

    // Only an explicit caller retry recovers the original permanent receipt.
    f.route.mode.store(PASS, Ordering::SeqCst);
    let (status, recovered) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("lost-reply"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{recovered}");
    assert_eq!(f.route.calls.lock().unwrap().len(), 2);
    assert_eq!(
        f.store()
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items,
        committed
    );
}

#[tokio::test]
async fn owner_routing_keeps_explicit_stale_control_revision_as_a_refusal() {
    let f = fixture(true).await;
    let mut body = intent(&f.original, json!({"type":"cancel"})).await;
    let current_revision = body["expected_agent_revision"].as_u64().unwrap();
    assert!(current_revision > 1);
    let stale_revision = current_revision - 1;
    body["expected_agent_revision"] = json!(stale_revision);
    let (status, response) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("stale-revision"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{response}");
    assert_eq!(response["error"]["code"], "response_control_not_admitted");
    let calls = f.route.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let OwnerCommandMutation::TargetedControl(submission) = &calls[0] else {
        panic!("wrong operation")
    };
    assert_eq!(submission.expected_agent_revision, stale_revision);
    assert_eq!(
        f.store()
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn owner_routing_failure_never_falls_back_to_local_control_publication() {
    for mode in [UNAVAILABLE, WRONG_VARIANT] {
        let f = fixture(true).await;
        f.route.mode.store(mode, Ordering::SeqCst);
        let body = intent(&f.original, json!({"type":"cancel"})).await;
        let before = f
            .store()
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items;
        let (status, response) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("uncertain-control"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{response}");
        assert_eq!(
            f.route.calls.lock().unwrap().len(),
            1,
            "never retry a route"
        );
        assert_eq!(
            f.store()
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items,
            before
        );
    }
}

#[tokio::test]
async fn owner_routing_preserves_public_control_refusals() {
    for (mode, expected, code) in [
        (
            NOT_ADMITTED,
            StatusCode::CONFLICT,
            "response_control_not_admitted",
        ),
        (
            UNSUPPORTED,
            StatusCode::UNPROCESSABLE_ENTITY,
            "capability_unsupported",
        ),
    ] {
        let f = fixture(true).await;
        f.route.mode.store(mode, Ordering::SeqCst);
        let body = intent(&f.original, json!({"type":"cancel"})).await;
        let (status, response) = call(
            &f.router,
            Method::POST,
            PATH,
            "owner-a",
            Some("refused-control"),
            Some(body),
        )
        .await;
        assert_eq!(status, expected, "{response}");
        assert_eq!(response["error"]["code"], code);
        assert_eq!(f.route.calls.lock().unwrap().len(), 1);
        assert_eq!(
            f.store()
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn owner_routing_requires_current_caller_and_current_owner_before_effects() {
    let f = fixture(true).await;
    let body = intent(&f.original, json!({"type":"cancel"})).await;
    let (status, _) = call(
        &f.router,
        Method::POST,
        PATH,
        "unrelated",
        Some("foreign-control"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(f.route.calls.lock().unwrap().is_empty());
    f.route.authority.revoke();
    let (status, response) = call(
        &f.router,
        Method::POST,
        PATH,
        "owner-a",
        Some("revoked-owner"),
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{response}");
    assert_eq!(f.route.calls.lock().unwrap().len(), 1);
    assert_eq!(
        f.store()
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}
