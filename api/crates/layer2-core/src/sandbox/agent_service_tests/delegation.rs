use super::*;
use crate::resource_access::{CallerContext, CredentialContext, CredentialSource, ResourceActor};
use crate::sandbox::store::AgentProvenanceStore;

fn delegated() -> CallerContext {
    CallerContext {
        caller: caller("owner", true),
        credential: CredentialContext {
            issuer: "fixture".into(),
            subject: SubjectId::new("owner"),
            credential_id: "verified-user-credential".into(),
        },
        credential_source: CredentialSource::delegated("approved-grant"),
        actor: ResourceActor::Api {
            subject: SubjectId::new("owner"),
            delegation_id: Some("approved-grant".into()),
        },
        memberships: vec![],
    }
}

#[tokio::test]
async fn verified_delegated_agent_creation_retains_exact_actor_in_durable_provenance() {
    let (service, store, profiles, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        3,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let context = delegated();
    let agent = service
        .create_authorized(
            &context,
            &workspace(WorkspaceStatus::Ready).id,
            request(reference.clone(), AgentOrigin::Manual),
        )
        .await
        .unwrap();
    assert_eq!(agent.owner, context.caller.subject);
    assert_eq!(agent.status, AgentStatus::Starting);
    assert_eq!(uses(&profiles, &reference.id).await, 1);
    let snapshot = store.agent_provenance_snapshot(&agent.id).await.unwrap();
    assert_eq!(snapshot.creation.unwrap().created_by, Some(context.actor));
    assert_eq!(
        store
            .get_worktree(&agent.worktree_id)
            .await
            .unwrap()
            .unwrap()
            .agent_id,
        Some(agent.id)
    );
}

#[tokio::test]
async fn delegated_creation_refuses_actor_only_forged_sources_and_foreign_identity() {
    let (service, store, _, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        3,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let context = delegated();
    let workspace_id = workspace(WorkspaceStatus::Ready).id;
    assert_eq!(
        service
            .create_as(
                &context.caller,
                &context.actor,
                &workspace_id,
                request(reference.clone(), AgentOrigin::Manual),
            )
            .await,
        Err(AgentServiceError::Forbidden)
    );
    for source in [
        CredentialSource::Retained,
        CredentialSource::Independent { expires_at: None },
        CredentialSource::Binding,
        CredentialSource::delegated("another-grant"),
    ] {
        let mut forged = context.clone();
        forged.credential_source = source;
        assert_eq!(
            service
                .create_authorized(
                    &forged,
                    &workspace_id,
                    request(reference.clone(), AgentOrigin::Manual),
                )
                .await,
            Err(AgentServiceError::Forbidden)
        );
    }
    let mut foreign = context.clone();
    foreign.credential.subject = SubjectId::new("foreign");
    let mut unprivileged = context;
    unprivileged.caller = caller("owner", false);
    for invalid in [foreign, unprivileged] {
        assert_eq!(
            service
                .create_authorized(
                    &invalid,
                    &workspace_id,
                    request(reference.clone(), AgentOrigin::Manual),
                )
                .await,
            Err(AgentServiceError::Forbidden)
        );
    }
    assert!(store
        .list_agents(&SubjectId::new("owner"), &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
    assert!(store
        .list_worktrees(&workspace_id, &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
