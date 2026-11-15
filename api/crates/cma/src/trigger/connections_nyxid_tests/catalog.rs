use super::*;

#[tokio::test]
async fn catalog_metadata_uses_exact_owner_visible_types_and_configured_setup_origin() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() = Reply::json(json!({ "services": [
        {"id":"provider-internal-id","slug":"github","name":"GitHub","service_type":"http","is_active":true,"base_url":"http://internal.example","oauth_client_secret":"not-projected"},
        {"slug":"workstation","name":"Development host","service_type":"ssh","is_active":true,"homepage_url":"https://untrusted.example/setup"},
        {"slug":"future","name":"Future service","service_type":"future_protocol","is_active":true},
        {"slug":"inactive","name":"Inactive service","service_type":"http","is_active":false},
        {"slug":"unreported","name":"Unreported service"}
    ] }));
    let tokens = Tokens::available();
    let broker = broker(&provider, tokens.clone());
    let owner = SubjectId::new("visitor-metadata");
    let services = [
        "github",
        "workstation",
        "future",
        "inactive",
        "unreported",
        "provider-internal-id",
        "missing",
    ]
    .map(ServiceId::new);
    let metadata = broker.describe_services(&owner, &services).await.unwrap();
    assert_eq!(metadata.len(), services.len());
    assert_eq!(metadata[0].display_name.as_deref(), Some("GitHub"));
    assert_eq!(metadata[0].connection_mode, ServiceConnectionMode::Hosted);
    assert_eq!(metadata[0].setup_url, None);
    assert_eq!(metadata[1].connection_mode, ServiceConnectionMode::Manual);
    assert_eq!(
        metadata[1].setup_url.as_deref(),
        Some("https://nyx.example/keys")
    );
    for item in &metadata[2..] {
        assert_eq!(item.connection_mode, ServiceConnectionMode::Unknown);
        assert_eq!(item.setup_url, None);
    }
    assert_eq!(metadata[5].display_name, None);
    assert_eq!(metadata[6].display_name, None);
    assert_eq!(*tokens.owners.lock().unwrap(), vec![owner]);
    assert_eq!(
        *provider.state.catalog_reads.lock().unwrap(),
        vec![Some(format!("Bearer {VISITOR_TOKEN}"))]
    );
    assert_eq!(provider.state.service_reads.lock().unwrap().len(), 1);
    assert!(provider.state.creates.lock().unwrap().is_empty());
    let printed = format!("{metadata:?}");
    for secret in [
        VISITOR_TOKEN,
        "not-projected",
        "internal.example",
        "untrusted.example",
    ] {
        assert!(!printed.contains(secret));
    }
}

#[tokio::test]
async fn existing_connections_link_to_the_exact_nyxid_repair_page() {
    let provider = Provider::start().await;
    *provider.state.catalog_reply.lock().unwrap() = Reply::json(json!({"services":[
        {"slug":"api-github","name":"GitHub","service_type":"http","is_active":true}
    ]}));
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[
            {"id":"github-existing","slug":"api-github","is_active":true}
        ]})));
    let metadata = broker(&provider, Tokens::available())
        .describe_services(
            &SubjectId::new("visitor-one"),
            &[
                ServiceId::new("api-github"),
                ServiceId::new("github-existing"),
            ],
        )
        .await
        .unwrap();
    for service in metadata {
        assert_eq!(service.display_name.as_deref(), Some("GitHub"));
        assert_eq!(service.connection_mode, ServiceConnectionMode::Manual);
        assert_eq!(
            service.setup_url.as_deref(),
            Some("https://nyx.example/keys/github-existing")
        );
    }
    assert!(provider.state.creates.lock().unwrap().is_empty());
}

#[tokio::test]
async fn active_github_credential_rows_without_material_stay_blocked() {
    let provider = Provider::start().await;
    provider
        .state
        .set_services_reply(Reply::json(json!({"services":[
            {"id":"github-existing","slug":"api-github","is_active":true}
        ]})));
    let broker = broker(&provider, Tokens::available());
    for (connection, expected) in [
        ("verifying", ServiceRequirementState::Missing),
        ("connecting", ServiceRequirementState::Missing),
        ("unknown", ServiceRequirementState::Unknown),
        ("connected", ServiceRequirementState::Satisfied),
    ] {
        *provider.state.readiness_reply.lock().unwrap() = Reply::json(json!({
            "revision":"nyxid-assistant-readiness.v1",
            "capabilities":[{"capabilityId":"api-github","connectionState":connection,"grantState":"missing"}]
        }));
        let requirements = broker
            .requirements(
                &SubjectId::new("visitor-one"),
                &[ServiceId::new("api-github")],
            )
            .await
            .unwrap();
        assert_eq!(requirements[0].state, expected);
    }
    *provider.state.readiness_reply.lock().unwrap() =
        Reply::json(json!({"revision":"future","capabilities":[]}));
    assert_eq!(
        broker
            .requirements(
                &SubjectId::new("visitor-one"),
                &[ServiceId::new("api-github")]
            )
            .await
            .unwrap_err(),
        malformed_error()
    );
}

#[tokio::test]
async fn catalog_metadata_rejects_duplicate_malformed_and_oversized_catalogs() {
    let provider = Provider::start().await;
    let broker = broker(&provider, Tokens::available());
    let row = json!({"slug":"github","name":"GitHub","service_type":"http","is_active":true});
    for catalog in [
        json!({"services":[row.clone(),row.clone()]}),
        json!({"services":[{"slug":"github","name":""}]}),
        json!({"services":[{"slug":"github","name":"x".repeat(257)}]}),
        json!({"services":[{"slug":"github\nsecret","name":"GitHub"}]}),
        json!({"services": vec![row; 1001]}),
        json!({"services":[], "ignored":"x".repeat(1024 * 1024)}),
        json!({"items":[]}),
    ] {
        *provider.state.catalog_reply.lock().unwrap() = Reply::json(catalog);
        assert_eq!(
            broker
                .describe_services(&SubjectId::new("visitor"), &[ServiceId::new("github")])
                .await,
            Err(malformed_error())
        );
    }
}

#[tokio::test]
async fn catalog_metadata_does_not_read_without_requirements_or_owner_credentials() {
    let provider = Provider::start().await;
    let tokens = Tokens::missing();
    let broker = broker(&provider, tokens.clone());
    let owner = SubjectId::new("visitor");
    assert!(broker
        .describe_services(&owner, &[])
        .await
        .unwrap()
        .is_empty());
    assert!(tokens.owners.lock().unwrap().is_empty());
    assert_eq!(
        broker
            .describe_services(&owner, &[ServiceId::new("github")])
            .await,
        Err(ServiceConnectionError::Refused)
    );
    assert!(provider.state.catalog_reads.lock().unwrap().is_empty());
}

#[test]
fn broker_requires_one_canonical_https_browser_origin() {
    for origin in [
        "http://nyx.example",
        "https://nyx.example/",
        "https://user@nyx.example",
        "https://nyx.example/connect",
        "https://nyx.example?next=elsewhere",
    ] {
        assert!(
            NyxidServiceConnectionBroker::new(
                reqwest::Client::new(),
                "https://identity.example",
                origin,
                Tokens::available(),
            )
            .is_err(),
            "accepted unsafe browser origin {origin}"
        );
    }
}
