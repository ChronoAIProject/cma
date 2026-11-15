use super::*;

#[tokio::test]
async fn handoff_creation_is_visitor_scoped_callback_free_and_exactly_fifteen_minutes() {
    let provider = Provider::start().await;
    let tokens = Tokens::available();

    let handoff = broker(&provider, tokens)
        .create_handoff(
            &SubjectId::new("visitor-one"),
            &ServiceId::new("github"),
            "intent-one",
        )
        .await
        .expect("handoff is created");

    assert_eq!(handoff.external_id, "link-one");
    assert_eq!(handoff.hosted_url, trusted_connect_url());
    assert_eq!(handoff.expires_at, CANONICAL_EXPIRES_AT);
    let creates = provider.state.creates.lock().expect("create requests lock");
    assert_eq!(creates.len(), 1);
    assert_eq!(
        creates[0].authorization.as_deref(),
        Some("Bearer visitor-access-secret")
    );
    assert_eq!(
        creates[0].body,
        json!({ "service_slug": "github", "expires_in": 900 })
    );
    assert!(creates[0].body.get("callback_url").is_none());
}

#[tokio::test]
async fn handoff_creation_uses_the_registered_static_callback_for_distinct_intents() {
    let provider = Provider::start().await;
    *provider.state.registered_callback.lock().unwrap() =
        Some("https://cma.example/connection-return".into());
    let broker = broker_with_callback(&provider, Tokens::available());
    for intent in ["intent-one", "intent-two"] {
        broker
            .create_handoff(
                &SubjectId::new("visitor-one"),
                &ServiceId::new("github"),
                intent,
            )
            .await
            .expect("the provider accepts the exact registered callback for every intent");
    }
    let creates = provider.state.creates.lock().expect("create requests lock");
    assert_eq!(creates.len(), 2);
    for request in creates.iter() {
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer visitor-access-secret")
        );
        assert_eq!(
            request.body,
            json!({
                "service_slug": "github",
                "expires_in": 900,
                "callback_url": "https://cma.example/connection-return"
            })
        );
    }
}

#[tokio::test]
async fn handoff_creation_rejects_foreign_insecure_or_malformed_connect_urls() {
    let provider = Provider::start().await;
    let invalid = [
        format!("http://nyx.example/connect/{CONNECT_TOKEN}"),
        format!("https://attacker.example/connect/{CONNECT_TOKEN}"),
        format!("https://nyx.example.evil/connect/{CONNECT_TOKEN}"),
        format!("https://user@nyx.example/connect/{CONNECT_TOKEN}"),
        format!("https://nyx.example/not-connect/{CONNECT_TOKEN}"),
        format!("https://nyx.example/connect/{CONNECT_TOKEN}/extra"),
        format!("https://nyx.example/connect/{CONNECT_TOKEN}?next=elsewhere"),
        format!("https://nyx.example/connect/{CONNECT_TOKEN}#fragment"),
        "https://nyx.example/connect/nyx_clk_short".to_owned(),
    ];

    for connect_url in invalid {
        provider
            .state
            .set_create_reply(create_reply(connect_url.clone()));
        assert_eq!(
            broker(&provider, Tokens::available())
                .create_handoff(
                    &SubjectId::new("visitor-one"),
                    &ServiceId::new("github"),
                    "intent-one",
                )
                .await,
            Err(malformed_error()),
            "accepted unsafe connect URL {connect_url}"
        );
    }
}

#[tokio::test]
async fn provider_terminal_states_map_without_polling_user_services() {
    let provider = Provider::start().await;
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    for (wire, expected) in [
        ("pending", BrokerHandoffState::Pending),
        ("expired", BrokerHandoffState::Expired),
        ("cancelled", BrokerHandoffState::Cancelled),
    ] {
        provider.state.set_link_reply(link_reply(wire));
        assert_eq!(
            broker
                .handoff_state(&owner, "link-one", &service)
                .await
                .expect("provider state maps"),
            expected
        );
    }
    assert!(provider
        .state
        .service_reads
        .lock()
        .expect("service requests lock")
        .is_empty());
}

#[tokio::test]
async fn completed_handoff_is_satisfied_only_for_the_exact_active_user_service() {
    let provider = Provider::start().await;
    provider.state.set_link_reply(Reply::json(json!({
        "id": "link-one",
        "status": "completed",
        "service_name": "GitHub",
        "service_slug": "github",
        "expires_at": PROVIDER_EXPIRES_AT,
        "connected_service": { "id": "user-service-one", "slug": "visitor-github" }
    })));
    let broker = broker(&provider, Tokens::available());
    let owner = SubjectId::new("visitor-one");
    let service = ServiceId::new("github");

    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            {
                "id": "user-service-one",
                "slug": "visitor-github",
                "is_active": true,
                "credential_source": { "type": "personal" }
            }
        ]
    })));
    assert_eq!(
        broker
            .handoff_state(&owner, "link-one", &service)
            .await
            .expect("active completion resolves"),
        BrokerHandoffState::Satisfied
    );

    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            {
                "id": "user-service-one",
                "slug": "visitor-github",
                "is_active": false,
                "credential_source": { "type": "personal" }
            }
        ]
    })));
    assert_eq!(
        broker
            .handoff_state(&owner, "link-one", &service)
            .await
            .expect("inactive completion resolves"),
        BrokerHandoffState::Failed
    );

    provider.state.set_services_reply(Reply::json(json!({
        "services": [
            {
                "id": "user-service-one",
                "slug": "visitor-github",
                "is_active": true,
                "credential_source": {
                    "type": "org",
                    "org_id": "org-one",
                    "org_name": "Read-only org",
                    "role": "viewer",
                    "allowed": false
                }
            }
        ]
    })));
    assert_eq!(
        broker
            .handoff_state(&owner, "link-one", &service)
            .await
            .expect("denied completion resolves"),
        BrokerHandoffState::Failed
    );
}
