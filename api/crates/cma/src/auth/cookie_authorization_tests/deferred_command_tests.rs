use layer2_core::sandbox::agent_io::*;
use layer2_core::{Authorization, Caller};

use super::*;

fn caller() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}
fn envelope(authorization: AgentAuthorizationRevision) -> AgentCommandEnvelope {
    let command = AgentCommand::message("fixture");
    AgentCommandEnvelope {
        continuation: None,
        schema_version: AGENT_IO_SCHEMA_VERSION,
        command_id: layer2_core::ids::AgentCommandId("cmd-fixture".into()),
        agent_id: layer2_core::AgentId("agent-fixture".into()),
        owner: AgentOwner::Person {
            subject: caller().subject.clone(),
        },
        actor: AgentActor::Human {
            subject: caller().subject.clone(),
        },
        source: AgentCommandSource {
            kind: AgentSourceKind::UserInterface,
            reference: "fixture".into(),
        },
        correlation_id: "correlation".into(),
        request_fingerprint: command.fingerprint().unwrap(),
        authorization,
        command,
    }
}

struct DeletingCommandOwner;
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for DeletingCommandOwner {
    async fn erasing(
        &self,
        _: &SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        Ok(true)
    }
}

#[tokio::test]
async fn delayed_command_refuses_a_currently_erasing_actor_and_owner() {
    let mut fixture = Fixture::new().await;
    fixture.seed().await;
    let reference = fixture
        .auth
        .personal_command_authority(&caller(), "must-not-leak-access-token")
        .await
        .unwrap();
    let command = envelope(reference);
    assert!(fixture.auth.authorize_command(&command).await.is_ok());
    Arc::get_mut(&mut fixture.auth)
        .unwrap()
        .set_fixture_standing(Arc::new(DeletingCommandOwner));
    assert_eq!(
        fixture.auth.authorize_command(&command).await,
        Err(CommandAuthorizationError::Revoked)
    );
}

#[tokio::test]
async fn admission_requires_exact_selected_credential_and_fresh_provider_permissions() {
    let fixture = Fixture::new().await;
    assert_eq!(
        fixture
            .auth
            .personal_command_authority(&caller(), "must-not-leak-access-token")
            .await,
        Err(CommandAuthorizationError::Revoked)
    );
    fixture.seed().await;
    assert_eq!(
        fixture
            .auth
            .personal_command_authority(&caller(), "bearer-with-no-custody")
            .await,
        Err(CommandAuthorizationError::Revoked)
    );
    let reference = fixture
        .auth
        .personal_command_authority(&caller(), "must-not-leak-access-token")
        .await
        .unwrap();
    assert!(!format!("{reference:?}").contains("must-not-leak"));
    *fixture.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
    assert_eq!(
        fixture
            .auth
            .personal_command_authority(&caller(), "must-not-leak-access-token")
            .await,
        Err(CommandAuthorizationError::Revoked)
    );
}

#[tokio::test]
async fn deferred_effect_rechecks_current_authority_and_never_trusts_saved_rbac() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let reference = fixture
        .auth
        .personal_command_authority(&caller(), "must-not-leak-access-token")
        .await
        .unwrap();
    let envelope = envelope(reference);
    let verified = fixture.auth.authorize_command(&envelope).await.unwrap();
    assert_eq!(verified, caller());
    *fixture.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
    assert_eq!(
        fixture.auth.authorize_command(&envelope).await,
        Err(CommandAuthorizationError::Revoked)
    );
    *fixture.control.introspection.write().await =
        Reply::json(json!({"active":false,"sub":"owner"}));
    assert_eq!(
        fixture.auth.authorize_command(&envelope).await,
        Err(CommandAuthorizationError::Revoked)
    );
    fixture.control.introspection.write().await.status = StatusCode::SERVICE_UNAVAILABLE;
    assert_eq!(
        fixture.auth.authorize_command(&envelope).await,
        Err(CommandAuthorizationError::Unavailable)
    );
}

#[tokio::test]
async fn new_sign_in_cannot_inherit_old_commands_even_if_subject_and_token_are_equal() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let reference = fixture
        .auth
        .personal_command_authority(&caller(), "must-not-leak-access-token")
        .await
        .unwrap();
    fixture.seed().await; // same subject and fixture token, new random incarnation
    assert_eq!(
        fixture.auth.authorize_command(&envelope(reference)).await,
        Err(CommandAuthorizationError::Revoked)
    );
}

#[tokio::test]
async fn token_rotation_within_incarnation_is_verified_and_does_not_rekey_authority() {
    let fixture = Fixture::new().await;
    let mut state = fixture.seed().await;
    let reference = fixture
        .auth
        .personal_command_authority(&caller(), "must-not-leak-access-token")
        .await
        .unwrap();
    state.token_generation += 1;
    state.revision += 1;
    state.tokens.as_mut().unwrap().access_token = SecretValue::new("rotated-fixture-token".into());
    fixture.store.put_subject(state).await.unwrap();
    assert_eq!(
        fixture
            .auth
            .authorize_command(&envelope(reference.clone()))
            .await
            .unwrap(),
        caller()
    );
    assert_eq!(
        fixture
            .auth
            .personal_command_authority(&caller(), "rotated-fixture-token")
            .await
            .unwrap(),
        reference
    );
}

#[tokio::test]
async fn owner_actor_channel_and_revision_tampering_cannot_select_another_credential() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let reference = fixture
        .auth
        .personal_command_authority(&caller(), "must-not-leak-access-token")
        .await
        .unwrap();
    for change in 0..7 {
        let mut command = envelope(reference.clone());
        match change {
            0 => {
                command.owner = AgentOwner::Organization {
                    subject: caller().subject,
                }
            }
            1 => {
                command.actor = AgentActor::Human {
                    subject: SubjectId::new("other"),
                }
            }
            2 => {
                command.actor = AgentActor::Channel {
                    bridge_id: "bridge".into(),
                    grant_id: "grant".into(),
                    platform: "telegram".into(),
                    chat_id: "chat".into(),
                    sender_id: "owner".into(),
                }
            }
            3 => command.authorization.policy_revision += 1,
            4 => command.authorization.grant_revision = Some(1),
            5 => command.authorization.authority_id = "personal:invented".into(),
            _ => {
                command.actor = AgentActor::Api {
                    subject: caller().subject,
                    delegation_id: Some("delegation".into()),
                }
            }
        }
        assert_eq!(
            fixture.auth.authorize_command(&command).await,
            Err(CommandAuthorizationError::Revoked)
        );
    }
}

#[tokio::test]
async fn replacement_during_provider_verification_never_returns_a_usable_caller() {
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let reference = fixture
        .auth
        .personal_command_authority(&caller(), "must-not-leak-access-token")
        .await
        .unwrap();
    let gate = Arc::new(Gate::default());
    fixture.control.introspection.write().await.gate = Some(gate.clone());
    let auth = fixture.auth.clone();
    let command = envelope(reference);
    let work = tokio::spawn(async move { auth.authorize_command(&command).await });
    gate.reached.notified().await;
    fixture.seed().await;
    gate.release.notify_one();
    assert_eq!(
        work.await.unwrap(),
        Err(CommandAuthorizationError::Unavailable)
    );
}

#[tokio::test]
async fn restricted_owner_projection_cannot_admit_deferred_personal_authority() {
    use base64::Engine as _;
    let fixture = Fixture::new().await;
    for claims in [
        json!({"relay":true}),
        json!({"sa":true}),
        json!({"delegated":true}),
        json!({"act":{"sub":"owner"}}),
    ] {
        let mut state = fixture.seed().await;
        let token = format!(
            "e30.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        );
        state.tokens.as_mut().unwrap().access_token = SecretValue::new(token.clone());
        fixture.store.put_subject(state).await.unwrap();
        let before = fixture.control.requests.load(Ordering::SeqCst);
        assert_eq!(
            fixture
                .auth
                .personal_command_authority(&caller(), &token)
                .await,
            Err(CommandAuthorizationError::Revoked)
        );
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    }
}

#[tokio::test]
async fn command_admission_binds_exact_request_context_and_keeps_owner_independent() {
    use layer2_core::resource_access::CallerContext;
    let fixture = Fixture::new().await;
    fixture.seed().await;
    let credential = "must-not-leak-access-token";
    let context = CallerContext {
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        caller: caller(),
        actor: AgentActor::Api {
            subject: caller().subject,
            delegation_id: None,
        },
        credential: fixture
            .auth
            .provider_credential_context(&caller().subject, credential)
            .unwrap(),
        memberships: Vec::new(),
    };
    let owner = AgentOwner::Person {
        subject: caller().subject,
    };
    let reference = fixture
        .auth
        .command_admission_authority(&context, credential, &owner)
        .await
        .unwrap();
    assert_eq!(
        reference,
        fixture
            .auth
            .personal_command_authority(&caller(), credential)
            .await
            .unwrap()
    );
    for resource_owner in [
        AgentOwner::Person {
            subject: SubjectId::new("foreign"),
        },
        AgentOwner::Organization {
            subject: SubjectId::new("org"),
        },
    ] {
        assert_eq!(
            fixture
                .auth
                .command_admission_authority(&context, credential, &resource_owner)
                .await
                .unwrap(),
            reference
        );
    }
    for change in 0..6 {
        let mut context = context.clone();
        let owner = owner.clone();
        let mut selected = credential;
        match change {
            0 => context.credential.issuer = "https://foreign.test".into(),
            1 => context.credential.credential_id = "foreign".into(),
            2 => context.credential.subject = SubjectId::new("foreign"),
            3 => {
                context.actor = AgentActor::Human {
                    subject: SubjectId::new("foreign"),
                }
            }
            4 => {
                context.actor = AgentActor::Api {
                    subject: caller().subject,
                    delegation_id: Some("delegation".into()),
                }
            }
            _ => selected = "another-credential",
        }
        let before = fixture.control.requests.load(Ordering::SeqCst);
        assert_eq!(
            fixture
                .auth
                .command_admission_authority(&context, selected, &owner)
                .await,
            Err(CommandAuthorizationError::Revoked)
        );
        assert_eq!(fixture.control.requests.load(Ordering::SeqCst), before);
    }
}
