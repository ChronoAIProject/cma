//! Ready-only Agent checks preserve fresh authority without waiting for labels.
use super::*;

fn gate(broker: &Broker) -> Arc<ReadGate> {
    let gate = Arc::new(ReadGate {
        requirements_started: AtomicUsize::new(0),
        metadata_started: AtomicUsize::new(0),
        requirements_release: tokio::sync::Semaphore::new(0),
        metadata_release: tokio::sync::Semaphore::new(0),
    });
    *broker.read_gate.lock().unwrap() = Some(gate.clone());
    gate
}

#[tokio::test]
async fn ready_agent_never_starts_metadata_or_its_credential_refresh() {
    let (browser, broker, _, caller) = fixture();
    let gate = gate(&broker);
    let check = browser.check(
        &caller,
        CheckRequest {
            target: target(),
            active: None,
        },
    );
    tokio::pin!(check);
    assert!(futures_util::poll!(&mut check).is_pending());
    assert_eq!(gate.requirements_started.load(Ordering::SeqCst), 1);
    assert_eq!(gate.metadata_started.load(Ordering::SeqCst), 0);
    gate.requirements_release.add_permits(1);
    let report = futures_util::poll!(&mut check)
        .map(Result::unwrap)
        .map(|report| {
            assert!(report.ready);
            assert_eq!(report.owner, "owner");
            assert_eq!(report.services.len(), 2);
            assert_eq!(report.services[0].service.as_str(), "api-github");
            assert_eq!(report.services[0].display_name, "Required NyxID service");
            assert_eq!(report.services[1].display_name, "My Codex subscription");
            assert!(report
                .services
                .iter()
                .all(|row| row.state == ServiceRequirementState::Satisfied
                    && row.connection_mode == "unknown"));
        });
    assert!(
        report.is_ready(),
        "catalog must not gate a freshly ready Agent"
    );
    assert_eq!(gate.metadata_release.available_permits(), 0);
    assert_eq!(gate.metadata_started.load(Ordering::SeqCst), 0);
    assert_eq!(broker.starts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn non_ready_agent_waits_for_real_repair_metadata() {
    for state in [
        ServiceRequirementState::Missing,
        ServiceRequirementState::Inactive,
        ServiceRequirementState::Unknown,
    ] {
        let (browser, broker, _, caller) = fixture();
        broker.rows.lock().unwrap()[0].state = state;
        let gate = gate(&broker);
        let check = browser.check(
            &caller,
            CheckRequest {
                target: target(),
                active: None,
            },
        );
        tokio::pin!(check);
        assert!(futures_util::poll!(&mut check).is_pending());
        gate.requirements_release.add_permits(1);
        assert!(futures_util::poll!(&mut check).is_pending());
        gate.metadata_release.add_permits(1);
        let report = check.await.unwrap();
        assert!(!report.ready);
        assert_eq!(report.services[0].state, state);
        assert_eq!(report.services[0].display_name, "GitHub");
        assert_eq!(report.services[0].connection_mode, "hosted");
    }
}

#[tokio::test]
async fn active_handoff_and_non_agent_checks_keep_full_metadata() {
    for request in [
        CheckRequest {
            target: target(),
            active: Some(active()),
        },
        CheckRequest {
            target: ConnectionTarget::Sandbox {
                id: layer2_core::SandboxId("sandbox".into()),
            },
            active: None,
        },
    ] {
        let (browser, broker, _, caller) = fixture();
        let gate = gate(&broker);
        let check = browser.check(&caller, request);
        tokio::pin!(check);
        assert!(futures_util::poll!(&mut check).is_pending());
        gate.requirements_release.add_permits(1);
        assert!(futures_util::poll!(&mut check).is_pending());
        gate.metadata_release.add_permits(1);
        let report = check.await.unwrap();
        assert_eq!(report.services[0].connection_mode, "hosted");
        if report.active_state.is_some() {
            assert!(!report.ready);
            assert_eq!(report.active_state, Some(BrokerHandoffState::Pending));
        }
    }
}

#[tokio::test]
async fn metadata_failure_never_replaces_fresh_requirement_verification() {
    let (browser, broker, _, caller) = fixture();
    broker.metadata_error.store(true, Ordering::SeqCst);
    let gate = gate(&broker);
    let check = browser.check(
        &caller,
        CheckRequest {
            target: target(),
            active: None,
        },
    );
    tokio::pin!(check);
    assert!(futures_util::poll!(&mut check).is_pending());
    assert_eq!(gate.metadata_started.load(Ordering::SeqCst), 0);
    gate.requirements_release.add_permits(1);
    assert!(check.await.unwrap().ready);
    assert_eq!(gate.metadata_started.load(Ordering::SeqCst), 0);

    // A subsequent check must observe the changed credential, not reuse readiness.
    *broker.read_gate.lock().unwrap() = None;
    broker.rows.lock().unwrap()[0].state = ServiceRequirementState::Missing;
    assert!(matches!(
        browser
            .check(
                &caller,
                CheckRequest {
                    target: target(),
                    active: None
                }
            )
            .await,
        Err(ConnectionError::Unavailable)
    ));
}

#[tokio::test]
async fn requirement_refusal_never_waits_for_or_trusts_metadata() {
    let (browser, broker, _, caller) = fixture();
    broker.requirements_error.store(true, Ordering::SeqCst);
    let gate = gate(&broker);
    let check = browser.check(
        &caller,
        CheckRequest {
            target: target(),
            active: None,
        },
    );
    tokio::pin!(check);
    assert!(futures_util::poll!(&mut check).is_pending());
    gate.requirements_release.add_permits(1);
    assert!(matches!(
        futures_util::poll!(&mut check),
        std::task::Poll::Ready(Err(ConnectionError::Unavailable))
    ));
}

#[tokio::test]
async fn reordered_or_foreign_satisfied_requirements_never_authorize_readiness() {
    for foreign in [false, true] {
        let (browser, broker, _, caller) = fixture();
        if foreign {
            broker.rows.lock().unwrap()[0].service = ServiceId::new("foreign-service");
        } else {
            broker.rows.lock().unwrap().reverse();
        }
        assert!(matches!(
            browser
                .check(
                    &caller,
                    CheckRequest {
                        target: target(),
                        active: None
                    }
                )
                .await,
            Err(ConnectionError::Unavailable)
        ));
    }
}

#[tokio::test]
async fn connection_ready_http_timing_with_delayed_broker() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let (browser, broker, _, caller) = fixture();
    *broker.delays.lock().unwrap() = (Duration::from_millis(180), Duration::from_millis(970));
    let auth = AuthState::in_memory(
        crate::auth::AuthMode::Trusted {
            subject: "owner".into(),
        },
        None,
    );
    let router = router(Arc::new(browser), auth);
    for (kind, id) in [("sandbox", "sandbox"), ("agent", "agent")] {
        let request = Request::builder()
            .method("POST")
            .uri("/auth/connections/check")
            .header(header::CONTENT_TYPE, "application/json")
            .extension(layer1_public::AuthenticatedSubject(caller.subject.clone()))
            .extension(caller.authorization.clone())
            .body(Body::from(
                serde_json::json!({"target":{"type":kind,"id":id}}).to_string(),
            ))
            .unwrap();
        let started = std::time::Instant::now();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let report: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(report["owner"], "owner");
        assert_eq!(report["ready"], true);
        assert_eq!(report["services"].as_array().unwrap().len(), 2);
        println!(
            "CONNECTION_READY_TIMING {}",
            serde_json::json!({
                "case": if kind == "agent" { "candidate_agent" } else { "full_metadata_non_agent_control" },
                "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
                "requirements_delay_ms": 180,
                "metadata_delay_ms": 970,
                "measurement": "in_process_http_route_with_injected_broker_delays_not_production_or_old_binary",
                "report": report,
            })
        );
    }
}
