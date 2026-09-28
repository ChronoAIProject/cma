use super::super::super::resource_fixture;
use super::*;
use layer2_core::resource_access::*;
use layer2_core::sandbox::{agent_io::*, control::*, store::*, AgentStatus};

async fn linked() -> (Fixture, String) {
    let f = fixture().await;
    *f.control.introspection.write().await = Reply::json(json!({
        "active":true,"sub":"owner","permissions":["cma:sandbox:use"]}));
    let (link, browser) = ready(&f).await;
    assert_eq!(approve(&f, &link, &browser).await.status(), StatusCode::OK);
    *f.control.organization_token.write().await = "rotated-fixture-token".into();
    (f, link)
}
async fn resources(
    f: &Fixture,
    owner: &str,
) -> (Arc<InMemorySandboxStore>, Arc<ResourceAccessService>) {
    let store = Arc::new(InMemorySandboxStore::default());
    let sandbox = resource_fixture::sandbox("sandbox", owner);
    store.create_sandbox(sandbox.clone()).await.unwrap();
    store
        .create_workspace(resource_fixture::workspace("workspace", "sandbox", owner))
        .await
        .unwrap();
    store
        .create_worktree(resource_fixture::worktree(
            "worktree",
            "workspace",
            "sandbox",
            "agent",
            owner,
        ))
        .await
        .unwrap();
    store
        .create_agent(resource_fixture::agent(
            "agent",
            "worktree",
            "workspace",
            "sandbox",
            owner,
            AgentStatus::Starting,
        ))
        .await
        .unwrap();
    if owner == "org" {
        store.set_resource_policy_for_test(ResourcePolicyRecord {
            sandbox_id: sandbox.id,
            sandbox_created_at: sandbox.created_at,
            owner: organization(),
            revision: 1,
            organization_evidence: Some(OrganizationOwnershipEvidence {
                issuer: "fixture".into(),
                organization: SubjectId::new("org"),
                admitted_by: ResourceActor::Api {
                    subject: SubjectId::new("owner"),
                    delegation_id: Some("fixture-admission-grant".into()),
                },
                verified_at: layer2_core::now_rfc3339(),
            }),
        });
    }
    let access = Arc::new(ResourceAccessService::new(store.clone(), f.auth.clone()));
    (store, access)
}
fn personal() -> ResourceOwner {
    ResourceOwner::Person {
        subject: SubjectId::new("owner"),
    }
}
fn organization() -> ResourceOwner {
    ResourceOwner::Organization {
        subject: SubjectId::new("org"),
    }
}
fn submission() -> CommandSubmission {
    CommandSubmission {
        agent_id: layer2_core::AgentId("agent".into()),
        command: AgentCommand::message("delegated command"),
        idempotency_key: "delegated-command".into(),
        expected_agent_revision: None,
        requested_model: None,
    }
}

#[tokio::test]
async fn delegated_command_retains_grant_through_admission_retry_execution_and_revival() {
    let (f, link) = linked().await;
    let (store, access) = resources(&f, "owner").await;
    let (context, token) = f
        .auth
        .context_for_grant(&link, &actor(), &personal())
        .await
        .unwrap();
    assert!(context.validate_principal());
    assert!(!context
        .caller
        .authorization
        .allows(Capability::ManageOwnBots));
    let service = AgentCommandService::new(store.clone(), access.clone(), f.auth.clone());
    let first = service
        .admit(
            &context,
            token.expose(),
            CommandSubmissionSource::Api,
            submission(),
        )
        .await
        .unwrap();
    let again = service
        .admit(
            &context,
            token.expose(),
            CommandSubmissionSource::Api,
            submission(),
        )
        .await
        .unwrap();
    assert_eq!(first.receipt.command_id, again.receipt.command_id);
    let command = store
        .get_command(&first.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        command.receipt.envelope.actor,
        ResourceActor::Api {
            subject: SubjectId::new("owner"),
            delegation_id: Some(link.clone())
        }
    );
    assert!(command
        .receipt
        .envelope
        .authorization
        .authority_id
        .starts_with("delegated:"));
    f.auth
        .authorize_resource_command(&access, &command.receipt.envelope)
        .await
        .unwrap();
    f.auth
        .authorize_command_revival(&access, &command)
        .await
        .unwrap()
        .require_command(&command)
        .unwrap();
    let json = serde_json::to_string(&command).unwrap();
    assert!(!json.contains(token.expose()) && !json.contains("rotated-fixture-refresh"));
    // A Caller-only compatibility path cannot supply delegated execution authority.
    assert!(access
        .authorize_personal_command_execution(&context.caller, &command.receipt.envelope)
        .await
        .is_err());
    f.auth.revoke_actor_link(&actor(), &link).await.unwrap();
    assert!(service
        .admit(
            &context,
            token.expose(),
            CommandSubmissionSource::Api,
            submission()
        )
        .await
        .is_err());
    assert!(f
        .auth
        .authorize_resource_command(&access, &command.receipt.envelope)
        .await
        .is_err());
    assert!(f
        .auth
        .authorize_command_revival(&access, &command)
        .await
        .is_err());
}

#[tokio::test]
async fn delegated_command_refresh_is_allowed_but_substituted_grant_and_login_are_not() {
    let (f, link) = linked().await;
    let (store, access) = resources(&f, "owner").await;
    let (context, token) = f
        .auth
        .context_for_grant(&link, &actor(), &personal())
        .await
        .unwrap();
    let service = AgentCommandService::new(store.clone(), access.clone(), f.auth.clone());
    let first = service
        .admit(
            &context,
            token.expose(),
            CommandSubmissionSource::Api,
            submission(),
        )
        .await
        .unwrap();
    let envelope = store
        .get_command(&first.receipt.command_id)
        .await
        .unwrap()
        .unwrap()
        .receipt
        .envelope;
    for change in ["grant", "authority", "source", "subject"] {
        let mut forged = envelope.clone();
        match change {
            "grant" => {
                forged.actor = ResourceActor::Api {
                    subject: SubjectId::new("owner"),
                    delegation_id: Some("0".repeat(32)),
                }
            }
            "authority" => forged.authorization.authority_id = "personal:pretend".into(),
            "source" => forged.source.kind = AgentSourceKind::UserInterface,
            _ => {
                forged.actor = ResourceActor::Api {
                    subject: SubjectId::new("another"),
                    delegation_id: Some(link.clone()),
                }
            }
        }
        assert!(
            f.auth
                .authorize_resource_command(&access, &forged)
                .await
                .is_err(),
            "{change}"
        );
    }
    let mut state = f
        .store
        .subject(&SubjectId::new("owner"))
        .await
        .unwrap()
        .unwrap();
    state.token_generation += 1;
    state.revision += 1;
    state.tokens.as_mut().unwrap().access_token = SecretValue::new("next-rotation".into());
    f.store.put_subject(state).await.unwrap();
    f.auth
        .authorize_resource_command(&access, &envelope)
        .await
        .unwrap();
    let (renewed, new_token) = f
        .auth
        .context_for_grant(&link, &actor(), &personal())
        .await
        .unwrap();
    let authority = f
        .auth
        .command_admission_authority(&renewed, new_token.expose(), &personal())
        .await
        .unwrap();
    assert_eq!(authority, envelope.authorization);
    f.seed().await;
    assert!(f
        .auth
        .authorize_resource_command(&access, &envelope)
        .await
        .is_err());
}

#[tokio::test]
async fn delegated_org_context_uses_current_per_operation_roles() {
    let (f, link) = linked().await;
    let (store, access) = resources(&f, "org").await;
    *f.control.organization.write().await = Reply::json(json!({"id":"org","your_role":"member"}));
    let (context, token) = f
        .auth
        .context_for_grant(&link, &actor(), &organization())
        .await
        .unwrap();
    access
        .authorize(
            &context,
            Some(token.expose()),
            &ResourceTarget::Agent {
                agent_id: layer2_core::AgentId("agent".into()),
            },
            ResourceCapability::ViewDetails,
            AccessPurpose::Read,
        )
        .await
        .unwrap();
    let service = AgentCommandService::new(store.clone(), access.clone(), f.auth.clone());
    assert!(service
        .admit(
            &context,
            token.expose(),
            CommandSubmissionSource::Api,
            submission()
        )
        .await
        .is_err());
    *f.control.organization.write().await = Reply::json(json!({"id":"org","your_role":"admin"}));
    let (context, token) = f
        .auth
        .context_for_grant(&link, &actor(), &organization())
        .await
        .unwrap();
    let first = service
        .admit(
            &context,
            token.expose(),
            CommandSubmissionSource::Api,
            submission(),
        )
        .await
        .unwrap();
    let command = store
        .get_command(&first.receipt.command_id)
        .await
        .unwrap()
        .unwrap();
    f.auth
        .authorize_resource_command(&access, &command.receipt.envelope)
        .await
        .unwrap();
    *f.control.organization.write().await = Reply::json(json!({"id":"org","your_role":"viewer"}));
    assert!(f
        .auth
        .authorize_resource_command(&access, &command.receipt.envelope)
        .await
        .is_err());
}

#[tokio::test]
async fn delegated_sandbox_control_retains_grant_and_rechecks_revocation() {
    let (f, link) = linked().await;
    let (store, access) = resources(&f, "owner").await;
    let (context, token) = f
        .auth
        .context_for_grant(&link, &actor(), &personal())
        .await
        .unwrap();
    let controls = SandboxControlService::new(store, access.clone(), f.auth.clone());
    let record = controls
        .admit(
            &context,
            token.expose(),
            SandboxControlSubmission {
                sandbox_id: layer2_core::SandboxId("sandbox".into()),
                command: SandboxControl::Pause,
                expected_revision: 1,
                idempotency_key: "delegated-control".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(record.envelope.actor, context.actor);
    f.auth
        .authorize_sandbox_control(&access, &record)
        .await
        .unwrap();
    f.auth.revoke_actor_link(&actor(), &link).await.unwrap();
    assert!(f
        .auth
        .authorize_sandbox_control(&access, &record)
        .await
        .is_err());
}

#[tokio::test]
async fn delegated_access_cannot_cross_actor_personal_owner_or_independent_credentials() {
    let (f, link) = linked().await;
    let mut wrong = actor();
    wrong.actor_id = "different-user".into();
    assert!(f
        .auth
        .context_for_grant(&link, &wrong, &personal())
        .await
        .is_err());
    assert!(f
        .auth
        .context_for_grant(
            &link,
            &actor(),
            &ResourceOwner::Person {
                subject: SubjectId::new("another")
            }
        )
        .await
        .is_err());
    let (mut context, token) = f
        .auth
        .context_for_grant(&link, &actor(), &personal())
        .await
        .unwrap();
    context.credential_source = CredentialSource::Independent { expires_at: None };
    assert!(!context.validate_principal());
    assert!(f
        .auth
        .command_admission_authority(&context, token.expose(), &personal())
        .await
        .is_err());
}
