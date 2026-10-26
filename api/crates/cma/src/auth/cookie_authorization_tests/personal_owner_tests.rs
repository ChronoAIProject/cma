use layer2_core::resource_access::CredentialSource;
use layer2_core::sandbox::agent_io::{
    AgentActor, AgentAuthorizationRevision, CommandAuthorizationError,
    PersonalCommandAdmissionAuthority,
};
use layer2_core::Caller;

use super::*;

async fn admit(fixture: &Fixture) -> AgentAuthorizationRevision {
    fixture
        .auth
        .personal_command_authority(
            &Caller::new(
                SubjectId::new("owner"),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
            "must-not-leak-access-token",
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn personal_owner_recovery_binds_human_context_to_the_verified_secret() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let authority = admit(&fixture).await;
    let owner = SubjectId::new("owner");
    let (context, credential) = fixture
        .auth
        .retained_personal_owner_authority(&owner, &authority)
        .await
        .unwrap();
    assert_eq!(context.caller.subject, owner);
    assert_eq!(
        context.actor,
        AgentActor::Human {
            subject: owner.clone()
        }
    );
    assert_eq!(context.credential_source, CredentialSource::Retained);
    assert!(context
        .caller
        .authorization
        .allows(Capability::UseOwnSandboxes));
    assert!(context.validate_principal());
    assert_eq!(
        context.credential,
        fixture
            .auth
            .provider_credential_context(&owner, credential.expose())
            .unwrap()
    );
    assert!(!format!("{credential:?}").contains(credential.expose()));
    assert_eq!(
        fixture
            .auth
            .personal_command_authority(&context.caller, credential.expose())
            .await
            .unwrap(),
        authority
    );
}

#[tokio::test]
async fn personal_owner_recovery_refreshes_only_within_the_approved_incarnation() {
    let fixture = Fixture::new().await;
    let mut state = fixture.seed().await;
    let authority = admit(&fixture).await;
    let owner = state.subject.clone();
    state.tokens.as_mut().unwrap().expires_at = SystemTime::UNIX_EPOCH;
    state.revision += 1;
    fixture.store.put_subject(state.clone()).await.unwrap();
    let (context, credential) = fixture
        .auth
        .retained_personal_owner_authority(&owner, &authority)
        .await
        .unwrap();
    assert_eq!(credential.expose(), "rotated-fixture-token");
    assert_eq!(
        context.credential,
        fixture
            .auth
            .provider_credential_context(&owner, credential.expose())
            .unwrap()
    );
    let refreshed = fixture.store.subject(&owner).await.unwrap().unwrap();
    assert_eq!(refreshed.incarnation, state.incarnation);
    assert!(refreshed.token_generation > state.token_generation);
    assert_eq!(
        fixture
            .auth
            .personal_command_authority(&context.caller, credential.expose())
            .await
            .unwrap(),
        authority
    );
    fixture.seed().await;
    assert!(matches!(
        fixture
            .auth
            .retained_personal_owner_authority(&owner, &authority)
            .await,
        Err(CommandAuthorizationError::Revoked)
    ));
}

#[tokio::test]
async fn personal_owner_recovery_never_borrows_another_identity_or_credential_kind() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let authority = admit(&fixture).await;
    let before = fixture.control.requests.load(Ordering::SeqCst);
    for prefix in ["api:", "binding:", "delegated:", "personal:invented"] {
        let mut different = authority.clone();
        different.authority_id = prefix.into();
        assert!(matches!(
            fixture
                .auth
                .retained_personal_owner_authority(&SubjectId::new("owner"), &different)
                .await,
            Err(CommandAuthorizationError::Revoked)
        ));
    }
    assert!(matches!(
        fixture
            .auth
            .retained_personal_owner_authority(&SubjectId::new("other"), &authority)
            .await,
        Err(CommandAuthorizationError::Revoked)
    ));
    assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    let mut state = fixture.seed().await;
    let authority = admit(&fixture).await;
    state.tokens = None;
    fixture.store.put_subject(state).await.unwrap();
    assert!(matches!(
        fixture
            .auth
            .retained_personal_owner_authority(&SubjectId::new("owner"), &authority)
            .await,
        Err(CommandAuthorizationError::Revoked)
    ));
}

#[tokio::test]
async fn personal_owner_recovery_rechecks_provider_revocation_and_live_permissions() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let authority = admit(&fixture).await;
    for reply in [
        json!({"active":true,"sub":"owner","permissions":[]}),
        json!({"active":false,"sub":"owner"}),
    ] {
        *fixture.control.introspection.write().await = Reply::json(reply);
        assert!(matches!(
            fixture
                .auth
                .retained_personal_owner_authority(&SubjectId::new("owner"), &authority)
                .await,
            Err(CommandAuthorizationError::Revoked)
        ));
    }
    fixture.control.introspection.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    assert!(matches!(
        fixture
            .auth
            .retained_personal_owner_authority(&SubjectId::new("owner"), &authority)
            .await,
        Err(CommandAuthorizationError::Unavailable)
    ));
}

#[tokio::test]
async fn personal_owner_recovery_fences_relogin_during_provider_verification() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let authority = admit(&fixture).await;
    let gate = Arc::new(Gate::default());
    fixture.control.introspection.write().await.gate = Some(gate.clone());
    let auth = fixture.auth.clone();
    let pending = tokio::spawn(async move {
        auth.retained_personal_owner_authority(&SubjectId::new("owner"), &authority)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), gate.reached.notified())
        .await
        .unwrap();
    fixture.seed().await;
    gate.release.notify_one();
    assert!(matches!(
        pending.await.unwrap(),
        Err(CommandAuthorizationError::Unavailable)
    ));
}

#[tokio::test]
async fn personal_owner_recovery_rejects_restricted_tokens_even_with_retained_custody() {
    use base64::Engine as _;
    let fixture = Fixture::new().await;
    for claims in [
        json!({"sa":true}),
        json!({"delegated":true}),
        json!({"act":{"sub":"owner"}}),
        json!({"relay":true}),
    ] {
        let mut state = fixture.seed().await;
        let authority = admit(&fixture).await;
        let token = format!(
            "e30.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        );
        state.revision += 1;
        state.token_generation += 1;
        state.tokens.as_mut().unwrap().access_token = SecretValue::new(token);
        fixture.store.put_subject(state).await.unwrap();
        let before = fixture.control.requests.load(Ordering::SeqCst);
        assert!(matches!(
            fixture
                .auth
                .retained_personal_owner_authority(&SubjectId::new("owner"), &authority)
                .await,
            Err(CommandAuthorizationError::Revoked)
        ));
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    }
}
