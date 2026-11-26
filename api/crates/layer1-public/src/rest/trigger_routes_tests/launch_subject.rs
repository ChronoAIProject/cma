use super::support_tests::*;
use axum::http::{Method, StatusCode};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

fn encoded(subject: &str) -> String {
    format!("v1.{}", URL_SAFE_NO_PAD.encode(subject))
}

#[tokio::test]
async fn mismatched_subject_never_reserves_an_operation_or_quota() {
    let app = TestApp::new().await;
    let mut req = request(
        Method::POST,
        "/api/v2/triggers/public/public-example/launches",
        Some(launch_body(false)),
        Some((OTHER, visitor_authority())),
        Some("saved-owner-command"),
    );
    req.headers_mut()
        .insert("x-cma-expected-subject", encoded(VISITOR).parse().unwrap());
    let (status, _, bytes) = raw_response(&app.router, req).await;
    assert_eq!(app.operation_reservations(), 0);
    assert_eq!(app.quota.reserve_calls(), 0);
    assert_eq!(app.launches.calls(), 0);
    assert_eq!(status, StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "command_subject_mismatch");
    assert_eq!(app.operation_reservations(), 0);
    assert_eq!(app.launches.calls(), 0);
    assert_eq!(app.quota.reserve_calls(), 0);
}

#[tokio::test]
async fn bound_route_recovers_a_legacy_receipt_without_recharging() {
    let app = TestApp::new().await;
    let trigger = create_trigger(&app, "always_new", "binding-create").await;
    let published = publish_trigger(&app, trigger["id"].as_str().unwrap(), "binding-publish").await;
    let public_id = published["public_id"].as_str().unwrap();
    let path = format!("/api/v1/triggers/public/{public_id}/launches");
    let first = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some("original-key"),
        ),
    )
    .await;
    assert_eq!(first.0, StatusCode::CREATED);
    assert!(!first.1.contains_key("x-cma-verified-subject"));
    let mut req = visitor_request(
        Method::POST,
        &path.replace("/v1/", "/v2/"),
        Some(launch_body(false)),
        Some("original-key"),
    );
    req.headers_mut()
        .insert("x-cma-expected-subject", encoded(VISITOR).parse().unwrap());
    let replay = raw_response(&app.router, req).await;
    assert_eq!(replay.0, first.0);
    assert_eq!(replay.2, first.2);
    assert_eq!(
        replay.1.get("x-cma-verified-subject").unwrap(),
        encoded(VISITOR).as_str()
    );
    assert_eq!(app.quota.reserve_calls(), 1);
    let original = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &path,
            Some(launch_body(false)),
            Some("original-key"),
        ),
    )
    .await;
    assert_eq!(original, first);
}

fn bound(
    path: &str,
    key: &str,
    subject: &str,
    expected: &str,
) -> axum::http::Request<axum::body::Body> {
    let mut req = request(
        Method::POST,
        path,
        Some(launch_body(false)),
        Some((subject, visitor_authority())),
        Some(key),
    );
    req.headers_mut()
        .insert("x-cma-expected-subject", encoded(expected).parse().unwrap());
    req
}

#[tokio::test]
async fn invalid_conditions_and_anonymous_requests_never_admit() {
    let app = TestApp::new().await;
    let path = "/api/v2/triggers/public/public-example/launches";
    for value in [
        None,
        Some("".into()),
        Some("v1.".into()),
        Some("v2.dmlzaXRvcg".into()),
        Some("v1.dmlzaXRvcg==".into()),
        Some("v1.dmlzaXRvch".into()),
        Some("v1._w".into()),
        Some(format!("v1.{}", "a".repeat(4094))),
    ] {
        let mut req = visitor_request(
            Method::POST,
            path,
            Some(launch_body(false)),
            Some("invalid"),
        );
        if let Some(value) = value {
            req.headers_mut()
                .insert("x-cma-expected-subject", value.parse().unwrap());
        }
        let (status, headers, bytes) = raw_response(&app.router, req).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["code"],
            "command_subject_condition_invalid"
        );
        assert!(!headers.contains_key("x-cma-verified-subject"));
    }
    let mut duplicate = bound(path, "duplicate", VISITOR, VISITOR);
    duplicate
        .headers_mut()
        .append("x-cma-expected-subject", encoded(VISITOR).parse().unwrap());
    assert_eq!(
        raw_response(&app.router, duplicate).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut anonymous = bound(path, "anonymous", VISITOR, VISITOR);
    anonymous.extensions_mut().clear();
    assert_eq!(
        raw_response(&app.router, anonymous).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(app.operation_reservations(), 0);
    assert_eq!(app.launches.calls(), 0);
    assert_eq!(app.quota.reserve_calls(), 0);
}

#[tokio::test]
async fn matching_condition_does_not_grant_authority_or_override_resource_identity() {
    use layer2_core::resource_access::{
        CallerContext, CredentialContext, CredentialSource, ResourceActor,
    };
    use layer2_core::{Authorization, Caller, SubjectId};
    let app = TestApp::new().await;
    let path = "/api/v2/triggers/public/public-example/launches";
    let mut forbidden = bound(path, "forbidden", VISITOR, VISITOR);
    forbidden.extensions_mut().insert(Authorization::none());
    let (status, headers, _) = raw_response(&app.router, forbidden).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(headers["x-cma-verified-subject"], encoded(VISITOR));
    let mut forged = bound(path, "forged", VISITOR, VISITOR);
    forged.extensions_mut().insert(CallerContext {
        caller: Caller::new(SubjectId::new(VISITOR), visitor_authority()),
        credential_source: CredentialSource::Retained,
        credential: CredentialContext {
            issuer: "https://identity.example.test".into(),
            subject: SubjectId::new(VISITOR),
            credential_id: "synthetic-binding".into(),
        },
        actor: ResourceActor::Human {
            subject: SubjectId::new(OTHER),
        },
        memberships: vec![],
    });
    let (status, headers, _) = raw_response(&app.router, forged).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!headers.contains_key("x-cma-verified-subject"));
    assert_eq!(app.operation_reservations(), 0);
    assert_eq!(app.launches.calls(), 0);
    assert_eq!(app.quota.reserve_calls(), 0);
}

#[tokio::test]
async fn canonical_unicode_and_maximum_subjects_are_not_normalized() {
    let app = TestApp::new().await;
    let path = "/api/v2/triggers/public/missing/launches";
    for subject in [
        "owner-甲".to_owned(),
        "a".repeat(3069),
        " Owner ".to_owned(),
    ] {
        let (status, headers, _) = raw_response(
            &app.router,
            bound(path, &format!("key-{}", subject.len()), &subject, &subject),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(headers["x-cma-verified-subject"], encoded(&subject));
    }
    let before = app.operation_reservations();
    let (status, _, _) = raw_response(
        &app.router,
        bound(path, "not-normalized", "Owner", " Owner "),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let large = "a".repeat(3070);
    assert_eq!(
        raw_response(&app.router, bound(path, "oversized", &large, &large))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(app.operation_reservations(), before);
}

async fn published_path(app: &TestApp) -> String {
    let trigger = create_trigger(app, "always_new", "bound-trigger").await;
    let published = publish_trigger(app, trigger["id"].as_str().unwrap(), "bound-publish").await;
    format!(
        "/api/v2/triggers/public/{}/launches",
        published["public_id"].as_str().unwrap()
    )
}

#[tokio::test]
async fn fresh_success_and_replay_reject_the_other_subject_before_store_access() {
    for start in [false, true] {
        let app = TestApp::new().await;
        app.broker
            .set_requirement(layer2_core::trigger_domain::ServiceRequirementState::Satisfied);
        let path = published_path(&app).await;
        let mut req = bound(&path, "saved-key", VISITOR, VISITOR);
        *req.body_mut() = axum::body::Body::from(launch_body(start).to_string());
        let first = raw_response(&app.router, req).await;
        assert_eq!(
            first.0,
            if start {
                StatusCode::ACCEPTED
            } else {
                StatusCode::CREATED
            }
        );
        assert_eq!(first.1["x-cma-verified-subject"], encoded(VISITOR));
        let reservations = app.operation_reservations();
        let domain_calls = app.launches.calls();
        let (status, headers, _) =
            raw_response(&app.router, bound(&path, "saved-key", OTHER, VISITOR)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(!headers.contains_key("x-cma-verified-subject"));
        assert_eq!(app.operation_reservations(), reservations);
        assert_eq!(app.launches.calls(), domain_calls);
        assert_eq!(app.quota.reserve_calls(), 1);
        let mut req = bound(&path, "saved-key", VISITOR, VISITOR);
        *req.body_mut() = axum::body::Body::from(launch_body(start).to_string());
        assert_eq!(raw_response(&app.router, req).await, first);
        assert_eq!(app.quota.reserve_calls(), 1);
    }
}

#[tokio::test]
async fn matching_recovery_preserves_pending_and_raw_byte_fences() {
    let app = TestApp::new().await;
    let path = published_path(&app).await;
    let legacy = path.replace("/v2/", "/v1/");
    let mut original = visitor_request(
        Method::POST,
        &legacy,
        Some(launch_body(false)),
        Some("wire"),
    );
    *original.body_mut() = axum::body::Body::from("{\"start_if_ready\":false,\"choice\":null}");
    assert_eq!(
        raw_response(&app.router, original).await.0,
        StatusCode::CREATED
    );
    let mut changed = bound(&path, "wire", VISITOR, VISITOR);
    *changed.body_mut() = axum::body::Body::from("{\"choice\":null,\"start_if_ready\":false}");
    let (status, headers, bytes) = raw_response(&app.router, changed).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(headers["x-cma-verified-subject"], encoded(VISITOR));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["code"],
        "idempotency_key_reused"
    );
    assert_eq!(app.quota.reserve_calls(), 1);

    app.fail_completion(Some(CompletionFault::BeforeWrite));
    let (_, _, bytes) = raw_response(
        &app.router,
        visitor_request(
            Method::POST,
            &legacy,
            Some(launch_body(false)),
            Some("uncertain"),
        ),
    )
    .await;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["code"],
        "store_failed"
    );
    let (status, headers, bytes) =
        raw_response(&app.router, bound(&path, "uncertain", VISITOR, VISITOR)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(headers["x-cma-verified-subject"], encoded(VISITOR));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["code"],
        "idempotency_in_flight"
    );
    assert_eq!(app.quota.reserve_calls(), 2);
}

#[tokio::test]
async fn expired_legacy_claim_remains_recovery_required_without_domain_work() {
    let app = TestApp::new().await;
    app.fail_operations(Ok(layer2_core::OperationReservation::RecoveryRequired));
    let (status, headers, bytes) = raw_response(
        &app.router,
        bound(
            "/api/v2/triggers/public/public-example/launches",
            "original-key",
            VISITOR,
            VISITOR,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(headers["x-cma-verified-subject"], encoded(VISITOR));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["code"],
        "idempotency_recovery_required"
    );
    assert_eq!(app.quota.reserve_calls(), 0);
    assert_eq!(app.launches.calls(), 0);
}
