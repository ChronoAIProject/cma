use super::*;

#[tokio::test]
async fn independent_api_admission_needs_no_browser_login_and_never_creates_one() {
    use layer2_core::sandbox::control::{
        SandboxControl, SandboxControlAuthorizer, SandboxControlService, SandboxControlSubmission,
    };
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
    let view = service(&fixture, auth.clone())
        .admit(
            &context,
            &credential,
            CommandSubmissionSource::Api,
            submission("no-login"),
        )
        .await
        .unwrap();
    let command = fixture
        .store
        .get_command(&view.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    auth.authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .unwrap();
    auth.authorize_command_revival(&fixture.access, &command)
        .await
        .unwrap();
    let parent = fixture
        .store
        .get_sandbox(&SandboxId("sandbox".into()))
        .await
        .unwrap()
        .unwrap();
    let control = SandboxControlService::new(
        fixture.store.clone(),
        Arc::new(fixture.access.clone()),
        auth.clone(),
    )
    .admit(
        &context,
        &credential,
        SandboxControlSubmission {
            sandbox_id: parent.id,
            command: SandboxControl::Pause,
            expected_revision: parent.revision,
            idempotency_key: "independent-pause".into(),
        },
    )
    .await
    .unwrap();
    auth.authorize_sandbox_control(&fixture.access, &control)
        .await
        .unwrap()
        .require_current()
        .unwrap();
    assert!(isolated
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn api_admission_refuses_missing_verified_expiry_and_mismatched_identity() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let credential = ordinary_token();
    for evidence in [
        json!({"active":true,"sub":"owner","permissions":["cma:sandbox:use"]}),
        json!({"active":true,"sub":"owner","token_type":"access","exp":1,"permissions":["cma:sandbox:use"]}),
        json!({"active":true,"sub":"owner","token_type":"refresh","exp":future_expiry(),"permissions":["cma:sandbox:use"]}),
        json!({"active":true,"sub":"owner","token_type":"access","exp":future_expiry(),"resource":["some-service"],"permissions":["cma:sandbox:use"]}),
    ] {
        let expired = evidence["exp"] == 1;
        *fixture.identity.control.introspection.write().await = Reply::json(evidence);
        if expired {
            assert!(
                identity::request_for_bearer(&fixture.identity.auth, credential.clone())
                    .await
                    .unwrap()
                    .is_none()
            );
            continue;
        }
        let context = external_context(&fixture, &credential).await;
        assert!(service(&fixture, fixture.identity.auth.clone())
            .admit(
                &context,
                &credential,
                CommandSubmissionSource::Api,
                submission("invalid-expiry"),
            )
            .await
            .is_err());
    }
    provider_expiry(&fixture, future_expiry()).await;
    let context = external_context(&fixture, &credential).await;
    for field in ["issuer", "subject", "credential", "actor", "source"] {
        let mut changed = context.clone();
        match field {
            "issuer" => changed.credential.issuer = "https://another-issuer.test".into(),
            "subject" => changed.caller.subject = SubjectId::new("another-subject"),
            "credential" => changed.credential.credential_id = "e".repeat(64),
            "actor" => {
                changed.actor = ResourceActor::Human {
                    subject: SubjectId::new("owner"),
                }
            }
            "source" => changed.credential_source = CredentialSource::Retained,
            _ => unreachable!(),
        }
        assert!(
            fixture
                .identity
                .auth
                .command_admission_authority(&changed, &credential, &fixture.envelope.owner)
                .await
                .is_err(),
            "{field}"
        );
    }
    assert!(fixture
        .store
        .command_work_page(&layer2_core::sandbox::store::PageRequest::new(20, None).unwrap())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn independent_custody_never_promotes_machine_scoped_or_sender_constrained_tokens() {
    let fixture = ResourceFixture::with_owner("owner").await;
    provider_expiry(&fixture, future_expiry()).await;
    let mut credentials = vec!["nyx_agent_key_opaque".into(), "opaque".into()];
    for restriction in [
        json!({"relay":true}),
        json!({"sa":true}),
        json!({"delegated":true}),
        json!({"act":{"sub":"owner"}}),
        json!({"cnf":{"jkt":"key"}}),
        json!({"resources":["service"]}),
        json!({"allowed_service_ids":["service"]}),
        json!({"allowed_node_ids":["node"]}),
        json!({"allow_all_services":false}),
        json!({"api_key_id":"agent-key"}),
        json!({"assistant_forward":true}),
    ] {
        let mut claims = json!({"token_type":"access","sub":"owner"});
        claims
            .as_object_mut()
            .unwrap()
            .extend(restriction.as_object().unwrap().clone());
        credentials.push(token(claims));
    }
    let valid = ordinary_token();
    let context = external_context(&fixture, &valid).await;
    for credential in credentials {
        let mut changed = context.clone();
        changed.credential = fixture
            .identity
            .auth
            .provider_credential_context(&changed.caller.subject, &credential)
            .unwrap();
        let before = fixture.identity.control.requests.load(Ordering::SeqCst);
        assert_eq!(
            fixture
                .identity
                .auth
                .command_admission_authority(&changed, &credential, &fixture.envelope.owner)
                .await,
            Err(CommandAuthorizationError::Revoked)
        );
        assert_eq!(
            fixture.identity.control.requests.load(Ordering::SeqCst),
            before
        );
    }
}

#[tokio::test]
async fn api_authority_cannot_cross_principal_issuer_expiry_or_actor_kind() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let credential = ordinary_token();
    provider_expiry(&fixture, future_expiry()).await;
    let command = admitted(&fixture, &credential, "immutable-identity").await;
    for change in ["actor", "human", "expiry", "id", "policy", "grant"] {
        let mut envelope = command.receipt.envelope.clone();
        match change {
            "actor" => {
                envelope.actor = AgentActor::Api {
                    subject: SubjectId::new("other"),
                    delegation_id: None,
                }
            }
            "human" => {
                envelope.actor = AgentActor::Human {
                    subject: SubjectId::new("owner"),
                }
            }
            "expiry" => envelope.authorization.authority_id.push('0'),
            "id" => {
                envelope.authorization.authority_id =
                    format!("api:{}:{}", "a".repeat(64), future_expiry())
            }
            "policy" => envelope.authorization.policy_revision += 1,
            "grant" => envelope.authorization.grant_revision = Some(1),
            _ => unreachable!(),
        }
        assert!(
            fixture
                .identity
                .auth
                .authorize_resource_command(&fixture.access, &envelope)
                .await
                .is_err(),
            "{change}"
        );
    }
    let mut mode = fixture.identity.auth.fixture_mode().clone();
    if let AuthMode::Provider { base_url, .. } = &mut mode {
        *base_url = "https://another-issuer.invalid".into();
    }
    let other = AuthState::with_store(mode, None, fixture.identity.store.clone());
    assert!(other
        .authorize_resource_command(&fixture.access, &command.receipt.envelope)
        .await
        .is_err());
}
