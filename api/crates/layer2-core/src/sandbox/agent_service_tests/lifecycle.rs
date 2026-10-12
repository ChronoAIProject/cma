use super::*;
use crate::resource_access::ResourceActor;
use crate::sandbox::store::AgentProvenanceStore;

#[tokio::test]
async fn lifecycle_replay_keeps_one_agent_worktree_log_and_original_api_actor() {
    let (service, store, profiles, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        1,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let owner = crate::Caller::new(
        SubjectId::new("owner"),
        crate::Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox]),
    );
    let actor = ResourceActor::Api {
        subject: owner.subject.clone(),
        delegation_id: Some("verified-original-grant".into()),
    };
    let input = request(
        reference.clone(),
        AgentOrigin::Lifecycle {
            operation_id: "lifecycle-one".into(),
        },
    );
    let workspace_id = workspace(WorkspaceStatus::Ready).id;
    assert!(matches!(
        service.create(&owner, &workspace_id, input.clone()).await,
        Err(AgentServiceError::InvalidRequest(_))
    ));
    let (left, right) = tokio::join!(
        service.create_for_lifecycle(&owner, Some(&actor), &workspace_id, input.clone()),
        service.create_for_lifecycle(&owner, Some(&actor), &workspace_id, input.clone())
    );
    let original = left.unwrap();
    assert_eq!(original, right.unwrap());
    let reopened = AgentService::new(
        store.clone(),
        profiles.clone(),
        TestAgentRuntimeRegistry::new(false),
        agent_capabilities(),
    );
    assert_eq!(
        reopened
            .create_for_lifecycle(&owner, Some(&actor), &workspace_id, input.clone())
            .await
            .unwrap(),
        original
    );
    assert_eq!(
        store
            .list_worktrees(&workspace_id, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(store.agent_log(&original.id, 0, 20).await.unwrap().len(), 1);
    assert_eq!(uses(&profiles, &reference.id).await, 1);
    let provenance = store
        .agent_provenance_snapshot(&original.id)
        .await
        .unwrap()
        .creation_provenance(&original.id)
        .unwrap();
    assert_eq!(provenance.created_by, Some(actor.clone()));
    assert_eq!(provenance.creation_origin, AgentCreationOrigin::Manual);
    assert_eq!(provenance.provisioning_origin, input.origin);
    let mut changed = input.clone();
    changed.title = Some("Different intent".into());
    assert!(matches!(
        reopened
            .create_for_lifecycle(&owner, Some(&actor), &workspace_id, changed)
            .await,
        Err(AgentServiceError::CreationConflict)
    ));
    let foreign_actor = ResourceActor::Human {
        subject: SubjectId::new("another-owner"),
    };
    assert!(matches!(
        reopened
            .create_for_lifecycle(&owner, Some(&foreign_actor), &workspace_id, input)
            .await,
        Err(AgentServiceError::Forbidden)
    ));
}
