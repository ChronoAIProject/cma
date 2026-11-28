use super::*;

#[tokio::test]
async fn selected_provider_dispositions_keep_known_status_without_private_details() {
    for (upstream, reason, disposition, known_status, attempts) in [
        (502, "http_status", "transient", Some(502), 3),
        (503, "http_status", "transient", Some(503), 3),
        (401, "rejected", "rejected", None, 1),
        (500, "unknown", "unavailable", None, 1),
    ] {
        let f = Fixture::new().await;
        {
            let mut metadata = f.auth.metadata.write().await;
            let metadata = metadata.as_mut().unwrap();
            metadata.userinfo_endpoint = Some(
                metadata
                    .userinfo_endpoint
                    .as_ref()
                    .unwrap()
                    .replace("/userinfo", "/api/v1/orgs/org-a/authorization"),
            );
        }
        // The existing authorization fixture supplies an actual HTTP response;
        // the verification reader selects the error before decoding its body.
        *f.control.reply.write().await = (
            StatusCode::from_u16(upstream).unwrap(),
            json!({"private":"must-not-leak-provider-body"}),
        );
        let ((status, body), events) =
            capture(context("sha256:status-fixture").scope(response(&f))).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["code"], "organization_authority_unavailable");
        let field = fields(&events, "principal_verification", reason);
        assert_eq!(field["result"], disposition);
        assert_eq!(field["status"], json!(known_status));
        assert_eq!(f.control.lookups.load(Ordering::SeqCst), attempts);
        assert_eq!(f.control.inventory_calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn local_admission_failure_is_distinct_from_provider_failure() {
    let f = Fixture::new().await;
    let held = f.auth.verification_slots.acquire_many(64).await.unwrap();
    let ((status, _), events) =
        capture(context("sha256:capacity-fixture").scope(response(&f))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let field = fields(&events, "principal_verification", "verification_capacity");
    assert_eq!(field["result"], "unavailable");
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 0);
    drop(held);
    let epochs: Vec<_> = (0..1024)
        .map(|i| {
            let key = f.auth.identity_store.key(&format!("fixture-{i}"));
            f.auth.identity_store.token_epoch(&key).unwrap()
        })
        .collect();
    let (result, events) = capture(choices(&f)).await;
    assert_eq!(result, Err(MembershipError::Unavailable));
    fields(
        &events,
        "principal_verification",
        "verification_fence_capacity",
    );
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 0);
    drop(epochs);
    let (result, events) = capture(choices(&f)).await;
    assert!(result.is_ok());
    assert!(events.is_empty());
}

#[tokio::test]
async fn principal_configuration_restriction_and_context_keep_original_refusals() {
    use super::super::super::principal_diagnostics::Operation;
    let trusted = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "fixture".into(),
        },
        None,
    );
    let (result, events) = capture(trusted.organization_principal(
        &caller(),
        "fixture-credential",
        Operation::Choices,
    ))
    .await;
    assert_eq!(result, Err(MembershipError::Unavailable));
    fields(&events, "principal_configuration", "missing_configuration");

    let mut f = Fixture::new().await;
    let (result, events) = capture(f.auth.organization_principal(
        &caller(),
        "header.invalid.signature",
        Operation::Membership,
    ))
    .await;
    assert_eq!(result, Err(MembershipError::NotMember));
    let field = fields(&events, "principal_restriction", "restricted_credential");
    assert_eq!(field["operation"], "membership");
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 0);

    if let AuthMode::Provider { base_url, .. } = &mut Arc::get_mut(&mut f.auth).unwrap().mode {
        *base_url = " ".into();
    }
    let (result, events) = capture(choices(&f)).await;
    assert_eq!(result, Err(MembershipError::InvalidEvidence));
    fields(&events, "principal_context", "invalid_context");
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.control.inventory_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn overall_deadline_retains_unknown_endpoint_and_fences_prior_evidence() {
    let f = Fixture::new().await;
    let metadata = f.auth.metadata().await.unwrap();
    let prior = identity_retry::verify_detailed(
        &f.auth,
        Some(&metadata),
        "fixture-credential",
        None,
        None,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    f.block_verification().await;
    let metadata = f.auth.metadata().await.unwrap();
    let result = identity_retry::verify_detailed(
        &f.auth,
        Some(&metadata),
        "fixture-credential",
        None,
        None,
        Duration::from_millis(20),
    )
    .await;
    let failure = result.err().unwrap();
    assert_eq!(failure.cause, VerificationCause::VerificationDeadline);
    assert_eq!(failure.reason(), "verification_deadline");
    assert_eq!(failure.status(), None);
    assert!(matches!(
        failure.error,
        ProviderVerificationError::Unavailable(_)
    ));
    assert!(!crate::auth::identity_lifetime::current(&f.auth, &prior));
}
