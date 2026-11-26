use super::*;
use layer2_core::sandbox::agent_access::AgentAccessError;

#[tokio::test]
async fn owner_api_consent_and_worker_restart_need_no_browser_login() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let credential = ordinary_token();
    provider_expiry(&fixture, future_expiry()).await;
    let isolated = Arc::new(MemoryAuthStore::default());
    let auth = AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        isolated.clone(),
    );
    let context = external_context(&fixture, &credential).await;
    let authority = auth
        .owner_service_access_authority(&context, &credential)
        .await
        .unwrap();
    assert!(authority.authority_id.starts_with("owner-api:"));
    auth.confirm_owner_service_scope(&context, &credential, &["github".into()])
        .await
        .unwrap();
    assert!(isolated
        .subject(&context.caller.subject)
        .await
        .unwrap()
        .is_none());
    let restarted = AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        isolated.clone(),
    );
    let (recovered, token) = restarted
        .retained_owner_service_authority(&context.caller.subject, &authority)
        .await
        .unwrap();
    assert_eq!(recovered, context);
    assert_eq!(token.expose(), credential);
    assert_eq!(
        restarted
            .owner_service_access_authority(&context, &credential)
            .await
            .unwrap(),
        authority
    );
    assert!(isolated
        .subject(&context.caller.subject)
        .await
        .unwrap()
        .is_none());
    assert!(!serde_json::to_string(&authority)
        .unwrap()
        .contains(&credential));
    for response in [
        json!({"active":false,"sub":"owner"}),
        json!({"active":true,"sub":"owner","token_type":"access","exp":future_expiry(),"permissions":[]}),
        json!({"active":true,"sub":"owner","token_type":"access","exp":1,"permissions":["cma:sandbox:use"]}),
    ] {
        *fixture.identity.control.introspection.write().await = Reply::json(response);
        assert!(restarted
            .retained_owner_service_authority(&context.caller.subject, &authority)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn runtime_delegated_and_machine_credentials_cannot_become_owner_consent() {
    let fixture = ResourceFixture::with_owner("owner").await;
    provider_expiry(&fixture, future_expiry()).await;
    let valid = ordinary_token();
    let context = external_context(&fixture, &valid).await;
    let mut values = vec![
        format!("nyxid_ag_{}", uuid::Uuid::new_v4().simple()),
        "opaque".into(),
    ];
    for restriction in [
        json!({"sa":true}),
        json!({"delegated":true}),
        json!({"act":{"sub":"owner"}}),
        json!({"relay":true}),
        json!({"allowed_node_ids":["node"]}),
        json!({"cnf":{"jkt":"key"}}),
    ] {
        let mut claims = json!({"sub":"owner","token_type":"access"});
        claims
            .as_object_mut()
            .unwrap()
            .extend(restriction.as_object().unwrap().clone());
        values.push(token(claims));
    }
    for credential in values {
        let mut changed = context.clone();
        changed.credential = fixture
            .identity
            .auth
            .provider_credential_context(&changed.caller.subject, &credential)
            .unwrap();
        assert_eq!(
            fixture
                .identity
                .auth
                .owner_service_access_authority(&changed, &credential)
                .await,
            Err(CommandAuthorizationError::Revoked)
        );
    }
    let mut delegated = context.clone();
    delegated.actor = AgentActor::Api {
        subject: context.caller.subject.clone(),
        delegation_id: Some("delegation".into()),
    };
    assert!(fixture
        .identity
        .auth
        .owner_service_access_authority(&delegated, &valid)
        .await
        .is_err());
    let mut wrong = context.clone();
    wrong.credential.credential_id = "a".repeat(64);
    assert!(fixture
        .identity
        .auth
        .owner_service_access_authority(&wrong, &valid)
        .await
        .is_err());
    let ordinary = fixture
        .identity
        .auth
        .command_admission_authority(&context, &valid, &fixture.envelope.owner)
        .await
        .unwrap();
    assert!(fixture
        .identity
        .auth
        .retained_owner_service_authority(&context.caller.subject, &ordinary)
        .await
        .is_err());
}

#[tokio::test]
async fn catalog_capability_does_not_allow_a_different_owners_credential_to_grant_services() {
    let fixture = ResourceFixture::with_owner("owner").await;
    provider_expiry(&fixture, future_expiry()).await;
    let credential = ordinary_token();
    let mut context = external_context(&fixture, &credential).await;
    context.caller.subject = layer2_core::SubjectId::new("other-owner");
    assert_eq!(
        fixture
            .identity
            .auth
            .owner_service_access_authority(&context, &credential)
            .await,
        Err(CommandAuthorizationError::Revoked)
    );
}

#[tokio::test]
async fn api_owner_gets_specific_missing_service_or_backing_authority_requirement() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let credential = ordinary_token();
    provider_expiry(&fixture, future_expiry()).await;
    let context = external_context(&fixture, &credential).await;
    assert_eq!(
        fixture
            .identity
            .auth
            .confirm_owner_service_scope(&context, &credential, &["missing-service".into()])
            .await,
        Err(AgentAccessError::ServiceConnectionRequired)
    );
    fixture
        .identity
        .control
        .scope_allowed
        .store(false, Ordering::SeqCst);
    assert_eq!(
        fixture
            .identity
            .auth
            .confirm_owner_service_scope(&context, &credential, &["github".into()])
            .await,
        Err(AgentAccessError::ServiceConnectionRequired)
    );
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_owner_api_scope_admission_and_credential_custody_survive_restart() {
    use crate::sandbox::OwnerTokenSource;
    use layer2_core::sandbox::agent_access::AgentAccessRequests;
    let fixture = ResourceFixture::with_owner("owner").await;
    let credential = ordinary_token();
    provider_expiry(&fixture, future_expiry()).await;
    let context = external_context(&fixture, &credential).await;
    let isolated = Arc::new(MemoryAuthStore::default());
    let auth = AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        isolated.clone(),
    );
    let (client, database, store, original) =
        crate::store::agent_access_requests::tests::approved_scoped_fixture().await;
    store
        .fail_access_request(original.lease.as_ref().unwrap(), "fixture_complete")
        .await
        .unwrap();
    let before = store
        .get_sandbox(&original.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let service = crate::agent_access::AgentAccessService::new(
        store.clone(),
        auth.clone(),
        Arc::new(layer2_core::sandbox::SandboxPolicy {
            baseline_services: vec![],
            ..Default::default()
        }),
    );
    let expected: Vec<_> = before
        .credential
        .effective_reaches()
        .iter()
        .map(ToString::to_string)
        .collect();
    let saved = service
        .replace_services(
            &context,
            &credential,
            &original.requester.agent_id,
            vec!["github".into()],
            expected.clone(),
            "owner-api-save",
        )
        .await
        .unwrap();
    let claimed = store
        .claim_access_request("owner-api-worker")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, saved.id);
    store
        .grant_live_access_request_services(claimed.lease.as_ref().unwrap())
        .await
        .unwrap();
    let after = store.get_sandbox(&before.id).await.unwrap().unwrap();
    let reopened = crate::store::MongoSandboxStore::open(&client, &database)
        .await
        .unwrap();
    let restarted = AuthState::with_store(
        fixture.identity.auth.fixture_mode().clone(),
        None,
        isolated.clone(),
    );
    let tokens = crate::sandbox::ServiceAccessOwnerTokens::new(restarted, Arc::new(reopened));
    assert_eq!(
        tokens
            .token_for_sandbox(&after)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        credential
    );
    assert_eq!(
        service
            .replace_services(
                &context,
                &credential,
                &original.requester.agent_id,
                vec!["github".into()],
                expected,
                "owner-api-save"
            )
            .await
            .unwrap()
            .id,
        saved.id
    );
    assert!(isolated
        .subject(&context.caller.subject)
        .await
        .unwrap()
        .is_none());
    *fixture.identity.control.introspection.write().await =
        Reply::json(json!({"active":false,"sub":"owner"}));
    assert!(
        tokens.token_for_sandbox(&after).await.is_err(),
        "revoked API authority must not borrow the browser login"
    );
    store
        .fail_access_request(claimed.lease.as_ref().unwrap(), "owner_authority_revoked")
        .await
        .unwrap();
    provider_expiry(&fixture, future_expiry()).await;
    let fresh = token(json!({"sub":"owner","token_type":"access","jti":"renewed-consent"}));
    let fresh_context = external_context(&fixture, &fresh).await;
    let renewed = service
        .replace_services(
            &fresh_context,
            &fresh,
            &original.requester.agent_id,
            vec!["github".into()],
            vec!["github".into()],
            "fresh-owner-consent",
        )
        .await
        .unwrap();
    assert_ne!(renewed.id, saved.id);
    assert_eq!(
        tokens
            .token_for_sandbox(&after)
            .await
            .unwrap()
            .unwrap()
            .expose(),
        fresh
    );
    assert_eq!(store.get_sandbox(&after.id).await.unwrap(), Some(after));
    client.database(&database).drop().await.unwrap();
}
