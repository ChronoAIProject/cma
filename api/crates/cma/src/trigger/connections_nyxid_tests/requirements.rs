use super::*;

#[tokio::test]
async fn requirements_preserve_order_and_honor_provider_credential_access() {
    let provider = Provider::start().await;
    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            {
                "id": "by-id",
                "slug": "alias",
                "is_active": true,
                "credential_source": { "type": "personal" }
            },
            {
                "id": "org-id",
                "slug": "org-service",
                "is_active": true,
                "credential_source": {
                    "type": "org",
                    "org_id": "org-one",
                    "org_name": "Allowed org",
                    "role": "member",
                    "allowed": true
                }
            },
            {
                "id": "denied-id",
                "slug": "denied",
                "is_active": true,
                "credential_source": {
                    "type": "org",
                    "org_id": "org-two",
                    "org_name": "Read-only org",
                    "role": "viewer",
                    "allowed": false
                }
            }
        ]
    })));
    let tokens = Tokens::available();
    let owner = SubjectId::new("visitor-one");
    let wanted = ["by-id", "org-service", "denied", "missing"].map(ServiceId::new);

    let requirements = broker(&provider, Arc::clone(&tokens))
        .requirements(&owner, &wanted)
        .await
        .expect("requirements resolve");

    assert_eq!(
        requirements
            .iter()
            .map(|requirement| (&requirement.service, requirement.state))
            .collect::<Vec<_>>(),
        vec![
            (&wanted[0], ServiceRequirementState::Satisfied),
            (&wanted[1], ServiceRequirementState::Satisfied),
            (&wanted[2], ServiceRequirementState::Unknown),
            (&wanted[3], ServiceRequirementState::Missing),
        ]
    );
    assert_eq!(
        *provider
            .state
            .service_reads
            .lock()
            .expect("service requests lock"),
        vec![Some(format!("Bearer {VISITOR_TOKEN}"))]
    );
    assert_eq!(
        *tokens.owners.lock().expect("token owner lock"),
        vec![owner]
    );
}

#[tokio::test]
async fn legacy_bare_listing_maps_inactive_and_missing_activity_to_closed_states() {
    let provider = Provider::start().await;
    provider.state.set_services_reply(Reply::json(json!([
        { "id": "inactive-id", "slug": "inactive", "is_active": false },
        { "id": "unknown-id", "slug": "unknown" }
    ])));
    let wanted = ["inactive", "unknown", "missing"].map(ServiceId::new);

    let requirements = broker(&provider, Tokens::available())
        .requirements(&SubjectId::new("visitor-one"), &wanted)
        .await
        .expect("legacy requirements resolve");

    assert_eq!(
        requirements
            .iter()
            .map(|requirement| requirement.state)
            .collect::<Vec<_>>(),
        vec![
            ServiceRequirementState::Inactive,
            ServiceRequirementState::Unknown,
            ServiceRequirementState::Missing,
        ]
    );
}

#[tokio::test]
async fn active_routing_rows_require_present_credentials_by_slug_and_id() {
    let provider = Provider::start().await;
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-routing-row","slug":"api-github","is_active":true},
        {"id":"ornn-row","slug":"ornn-api","is_active":true}
    ] })));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor-one");
    let services = ["api-github", "github-routing-row", "ornn-api"].map(ServiceId::new);
    for (reply, expected) in [
        (
            Reply::json(
                json!({"id":"github-routing-row","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":true}),
            ),
            ServiceRequirementState::Missing,
        ),
        (
            Reply::error(StatusCode::NOT_FOUND, "private provider detail"),
            ServiceRequirementState::Missing,
        ),
        (
            Reply::json(
                json!({"id":"github-routing-row","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":false}),
            ),
            ServiceRequirementState::Satisfied,
        ),
    ] {
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert("github-routing-row".into(), reply);
        let requirements = broker.requirements(&owner, &services).await.unwrap();
        assert_eq!(
            requirements.iter().map(|r| r.state).collect::<Vec<_>>(),
            vec![expected, expected, ServiceRequirementState::Satisfied]
        );
    }
    assert_eq!(
        *provider.state.credential_reads.lock().unwrap(),
        vec![Some(format!("Bearer {VISITOR_TOKEN}")); 9]
    );
}

#[tokio::test]
async fn credential_metadata_failures_never_look_like_satisfied_connections() {
    let provider = Provider::start().await;
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-row","slug":"api-github","is_active":true}
    ] })));
    let broker = broker(&provider, Tokens::available());
    for reply in [
        Reply::error(StatusCode::TOO_MANY_REQUESTS, "private provider response"),
        Reply::error(StatusCode::SERVICE_UNAVAILABLE, "private provider response"),
        Reply::json(
            json!({"id":"another-user-service","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":false}),
        ),
        Reply::json(
            json!({"id":"github-row","slug":"api-github","is_active":true,"status":"active","connected":true}),
        ),
    ] {
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert("github-row".into(), reply);
        let error = broker
            .requirements(
                &SubjectId::new("visitor-one"),
                &[ServiceId::new("api-github")],
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ServiceConnectionError::Unavailable(_) | ServiceConnectionError::Malformed(_)
        ));
        assert!(!error.to_string().contains("private provider"));
    }
    provider.state.set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-row","slug":"api-github","is_active":true,"credential_source":{"type":"org","allowed":false}}
    ] })));
    let requirements = broker
        .requirements(
            &SubjectId::new("visitor-one"),
            &[ServiceId::new("api-github")],
        )
        .await
        .unwrap();
    assert_eq!(requirements[0].state, ServiceRequirementState::Unknown);
    assert_eq!(
        provider.state.credential_reads.lock().unwrap().len(),
        4,
        "denied organization credentials must not be probed"
    );
}

#[tokio::test]
async fn completed_handoff_still_requires_present_credentials() {
    let provider = Provider::start().await;
    provider.state.set_link_reply(Reply::json(json!({
        "id":"link-one","status":"completed","service_slug":"api-github",
        "expires_at":PROVIDER_EXPIRES_AT,
        "connected_service":{"id":"github-row","slug":"api-github"}
    })));
    provider
        .state
        .set_services_reply(Reply::json(json!({ "services": [
        {"id":"github-row","slug":"api-github","is_active":true}
    ] })));
    let broker = broker(&provider, Tokens::available());
    provider.state.credential_replies.lock().unwrap().insert("github-row".into(),
        Reply::json(json!({"id":"github-row","slug":"api-github","is_active":true,"status":"active","connected":true,"credential_missing":true})));
    assert_eq!(
        broker
            .handoff_state(
                &SubjectId::new("visitor-one"),
                "link-one",
                &ServiceId::new("api-github")
            )
            .await
            .unwrap(),
        BrokerHandoffState::Failed
    );
    provider
        .state
        .credential_replies
        .lock()
        .unwrap()
        .remove("github-row");
    assert_eq!(
        broker
            .handoff_state(
                &SubjectId::new("visitor-one"),
                "link-one",
                &ServiceId::new("api-github")
            )
            .await
            .unwrap(),
        BrokerHandoffState::Satisfied
    );
}

#[tokio::test]
async fn every_required_service_checks_credential_state_before_becoming_satisfied() {
    let provider = Provider::start().await;
    let cases = [
        (
            "pending",
            json!({"status":"pending_auth"}),
            ServiceRequirementState::Missing,
        ),
        (
            "expired",
            json!({"connection_status":"expired"}),
            ServiceRequirementState::Missing,
        ),
        (
            "empty",
            json!({"credential_missing":true}),
            ServiceRequirementState::Missing,
        ),
        (
            "disconnected",
            json!({"connected":false}),
            ServiceRequirementState::Missing,
        ),
        (
            "disabled",
            json!({"is_active":false}),
            ServiceRequirementState::Inactive,
        ),
        (
            "unknown",
            json!({"status":"future"}),
            ServiceRequirementState::Unknown,
        ),
        (
            "internal",
            json!({"auth_method":"none","credential_missing":false}),
            ServiceRequirementState::Satisfied,
        ),
        (
            "node",
            json!({"node_id":"node-one","node_status":"offline"}),
            ServiceRequirementState::Inactive,
        ),
        ("active", json!({}), ServiceRequirementState::Satisfied),
    ];
    let rows = cases
        .iter()
        .map(|(id, _, _)| json!({"id":id,"slug":id,"is_active":true}))
        .collect::<Vec<_>>();
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":rows})));
    for (id, fields, _) in &cases {
        let mut key = json!({"id":id,"slug":id,"is_active":true,"status":"active","connected":true,"credential_missing":false});
        key.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        provider
            .state
            .credential_replies
            .lock()
            .unwrap()
            .insert(id.to_string(), Reply::json(key));
    }
    let services = cases
        .iter()
        .map(|(id, _, _)| ServiceId::new(*id))
        .collect::<Vec<_>>();
    let requirements = broker(&provider, Tokens::available())
        .requirements(&SubjectId::new("visitor-one"), &services)
        .await
        .unwrap();
    assert_eq!(
        requirements.iter().map(|r| r.state).collect::<Vec<_>>(),
        cases.iter().map(|(_, _, state)| *state).collect::<Vec<_>>()
    );
}
