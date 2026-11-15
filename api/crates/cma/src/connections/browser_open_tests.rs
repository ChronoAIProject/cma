use super::*;

fn request() -> CheckRequest {
    CheckRequest {
        target: target(),
        active: None,
    }
}

#[tokio::test]
async fn open_readiness_uses_warm_evidence_without_waiting_for_background_check() {
    let (browser, broker, _, caller) = fixture();
    assert_eq!(
        browser.open(&caller, request()).await.unwrap().verification,
        "fresh"
    );
    let gate = ready::gate(&broker);
    let report = browser.open(&caller, request()).await.unwrap();
    assert!(report.report.ready);
    assert_eq!(report.verification, "cached");
    assert!(report.valid_for_ms <= 30_000);
    assert_eq!(report.report.owner, caller.subject.as_str());
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(gate.requirements_started.load(Ordering::SeqCst), 1);
    for _ in 0..8 {
        assert_eq!(
            browser.open(&caller, request()).await.unwrap().verification,
            "cached"
        );
    }
    assert_eq!(gate.requirements_started.load(Ordering::SeqCst), 1);
    assert_eq!(gate.metadata_started.load(Ordering::SeqCst), 0);
    gate.requirements_release.add_permits(1);
    browser.readiness.idle().await;
}

#[tokio::test]
async fn open_readiness_cache_never_authorizes_a_send_check_after_revocation() {
    let (browser, broker, _, caller) = fixture();
    assert!(browser.check(&caller, request()).await.unwrap().ready);
    broker.rows.lock().unwrap()[0].state = ServiceRequirementState::Inactive;
    // Open may use bounded presentation evidence while the async check runs.
    assert_eq!(
        browser.open(&caller, request()).await.unwrap().verification,
        "cached"
    );
    // The Send endpoint always performs the current broker check, never the cache.
    let fresh = browser.check(&caller, request()).await.unwrap();
    assert!(!fresh.ready);
    assert_eq!(fresh.services[0].state, ServiceRequirementState::Inactive);
    browser.readiness.idle().await;
    assert!(!browser.open(&caller, request()).await.unwrap().report.ready);
}

#[tokio::test]
async fn open_readiness_background_refusal_invalidates_and_never_refreshes_identity() {
    let (browser, broker, _, caller) = fixture();
    browser.check(&caller, request()).await.unwrap();
    broker.requirements_error.store(true, Ordering::SeqCst);
    assert_eq!(
        browser.open(&caller, request()).await.unwrap().verification,
        "cached"
    );
    browser.readiness.idle().await;
    assert!(matches!(
        browser.open(&caller, request()).await,
        Err(ConnectionError::Unavailable)
    ));
    let foreign = Caller::new(SubjectId::new("foreign"), Authorization::none());
    assert!(matches!(
        browser.open(&foreign, request()).await,
        Err(ConnectionError::NotFound)
    ));
}

#[tokio::test(start_paused = true)]
async fn open_readiness_expired_or_active_handoff_requires_fresh_evidence() {
    let (browser, broker, _, caller) = fixture();
    browser.check(&caller, request()).await.unwrap();
    tokio::time::advance(Duration::from_secs(30)).await;
    broker.rows.lock().unwrap()[0].state = ServiceRequirementState::Missing;
    let report = browser.open(&caller, request()).await.unwrap();
    assert_eq!(report.verification, "fresh");
    assert!(!report.report.ready);
    assert!(matches!(
        browser
            .open(
                &caller,
                CheckRequest {
                    target: target(),
                    active: Some(active())
                }
            )
            .await,
        Err(ConnectionError::Invalid)
    ));
}

#[tokio::test]
async fn open_readiness_background_malformed_evidence_cannot_keep_cached_success() {
    for malformed in [0, 1, 2] {
        let (browser, broker, _, caller) = fixture();
        browser.check(&caller, request()).await.unwrap();
        {
            let mut rows = broker.rows.lock().unwrap();
            match malformed {
                0 => {
                    rows.pop();
                }
                1 => {
                    rows[0].service = ServiceId::new("unrequested");
                }
                _ => rows.reverse(),
            }
        }
        browser.open(&caller, request()).await.unwrap();
        browser.readiness.idle().await;
        assert!(matches!(
            browser.open(&caller, request()).await,
            Err(ConnectionError::Unavailable)
        ));
    }
}

#[tokio::test]
async fn open_readiness_http_timings_and_no_store_contract() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (browser, broker, _, caller) = fixture();
    *broker.delays.lock().unwrap() = (Duration::from_millis(1100), Duration::ZERO);
    let browser = Arc::new(browser);
    let app = router(browser.clone());
    for round in 0..5 {
        for path in ["check", "open"] {
            let call = Request::builder()
                .method("POST")
                .uri(format!("/auth/connections/{path}"))
                .header(header::CONTENT_TYPE, "application/json")
                .extension(layer1_public::AuthenticatedSubject(caller.subject.clone()))
                .extension(caller.authorization.clone())
                .body(Body::from(
                    serde_json::json!({"target":{"type":"agent","id":"agent"}}).to_string(),
                ))
                .unwrap();
            let started = std::time::Instant::now();
            let response = app.clone().oneshot(call).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            let report: serde_json::Value =
                serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                    .unwrap();
            assert_eq!(report["ready"], true);
            if path == "open" {
                assert_eq!(report["verification"], "cached");
            }
            println!(
                "OPEN_READINESS_TIMING {}",
                serde_json::json!({"path":path,"round":round,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"report":report,"measurement":"actual_in_process_http_with_1100ms_broker_fixture_not_production"})
            );
        }
        browser.readiness.idle().await;
    }
}
