use super::*;
use crate::resource_access::{ResourceActor, ResourceOwner};
use crate::sandbox::store::AgentProvenanceStore;
use crate::sandbox::{AgentCreationOrigin, AgentCreationProvenance};

fn actor(subject: &str) -> ResourceActor {
    ResourceActor::Api {
        subject: SubjectId::new(subject),
        delegation_id: None,
    }
}

#[tokio::test]
async fn provenance_records_creator_separately_and_retry_never_relabels_it() {
    let store = ready_store(3).await;
    let mut request = admission("provenance-retry");
    request.created_by = Some(actor("creating-principal"));
    let first = created(store.admit_agent(request.clone()).await.unwrap());
    let before = store
        .agent_provenance_snapshot(&first.agent.id)
        .await
        .unwrap()
        .creation_provenance(&first.agent.id)
        .unwrap();
    assert_eq!(before.created_by, request.created_by);
    assert_eq!(
        before.owner,
        ResourceOwner::Person {
            subject: request.owner.clone()
        }
    );
    assert_eq!(before.creation_origin, AgentCreationOrigin::Manual);
    request.created_by = Some(actor("retrying-principal"));
    assert!(matches!(
        store.admit_agent(request).await.unwrap(),
        AgentAdmissionResult::Existing(_)
    ));
    let state = store.lock();
    assert_eq!(state.agent_creation_provenance.len(), 1);
    assert_eq!(state.agent_creation_provenance[&first.agent.id], before);
}

#[tokio::test]
async fn provenance_survives_lifecycle_profile_changes_and_refuses_recycled_identity() {
    let store = ready_store(3).await;
    let request = admission("provenance-retention");
    let first = created(store.admit_agent(request.clone()).await.unwrap());
    let before = store
        .agent_provenance_snapshot(&first.agent.id)
        .await
        .unwrap()
        .creation
        .unwrap();
    {
        let mut state = store.lock();
        let agent = state.agents.get_mut(&first.agent.id).unwrap();
        agent.status = AgentStatus::Deleted;
        agent.deleted_at = Some("2026-09-06T00:00:00Z".into());
        agent.profile.revision += 1;
        agent.revision += 1;
        agent.conversation.runtime_generation = Some(2);
        agent.conversation.conversation_id = Some("retained-conversation".into());
    }
    assert_eq!(
        store
            .agent_provenance_snapshot(&first.agent.id)
            .await
            .unwrap()
            .creation_provenance(&first.agent.id)
            .unwrap(),
        before
    );
    {
        let mut state = store.lock();
        state.agents.remove(&first.agent.id);
        state.worktrees.remove(&first.worktree.id);
        state.agent_logs.remove(&first.agent.id);
    }
    let mut recycled = request;
    recycled.creation_key = "another-creation".into();
    recycled.worktree_id = WorktreeId::generate();
    assert_eq!(
        store.admit_agent(recycled).await.unwrap_err(),
        SandboxStoreError::Duplicate
    );
    assert_eq!(
        store.lock().agent_creation_provenance[&first.agent.id],
        before
    );
}

#[tokio::test]
async fn provenance_rolls_back_when_a_later_creation_event_cannot_commit() {
    let store = ready_store(3).await;
    store
        .lock()
        .sandboxes
        .get_mut(&sandbox(3).id)
        .unwrap()
        .revision = u64::MAX;
    assert!(store
        .admit_agent(admission("provenance-rollback"))
        .await
        .is_err());
    let state = store.lock();
    assert!(state.agent_creation_provenance.is_empty());
    assert!(state.agents.is_empty());
    assert!(state.worktrees.is_empty());
}

#[tokio::test]
async fn provenance_trigger_reuse_retains_original_profile_origin() {
    let store = ready_store(3).await;
    let mut automatic = admission("workspace-profile");
    automatic.creation_key = format!("workspace-profile:{}:0", automatic.workspace_id);
    store
        .lock()
        .workspaces
        .get_mut(&automatic.workspace_id)
        .unwrap()
        .profile
        .content
        .agent_profiles = vec![crate::profiles::AgentProfileRef::new(
        automatic.profile.id.clone(),
        automatic.profile.revision,
    )];
    automatic.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    let first = created(store.admit_agent(automatic.clone()).await.unwrap());
    let before = store
        .agent_provenance_snapshot(&first.agent.id)
        .await
        .unwrap()
        .creation
        .unwrap();
    assert_eq!(before.creation_origin, AgentCreationOrigin::LegacyUnknown);
    assert_eq!(before.provisioning_origin, automatic.origin);
    let mut launch = admission("trigger-reuse");
    launch.origin = AgentOrigin::Trigger {
        trigger_id: "trg-reuse".into(),
        launch_id: "lch-reuse".into(),
    };
    launch.created_by = Some(actor("launcher"));
    let result = store.admit_agent_reusing_profile(launch).await.unwrap();
    assert_eq!(result.creation().agent.id, first.agent.id);
    assert_eq!(
        store
            .agent_provenance_snapshot(&first.agent.id)
            .await
            .unwrap()
            .creation
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn provenance_legacy_inference_requires_exact_creation_evidence() {
    let store = ready_store(3).await;
    for trigger in [false, true] {
        let mut request = admission("unused");
        if trigger {
            request.origin = AgentOrigin::Trigger {
                trigger_id: "trg-one".into(),
                launch_id: "lch-one".into(),
            };
            request.creation_key = format!(
                "trigger:{}",
                crate::profiles::content_digest(&request.origin).unwrap()
            );
        } else {
            request.creation_key = format!("manual:{}", request.agent_id);
        }
        request.created_by = Some(actor("creator"));
        let first = created(store.admit_agent(request).await.unwrap());
        store
            .lock()
            .agent_creation_provenance
            .remove(&first.agent.id);
        let mut snapshot = store
            .agent_provenance_snapshot(&first.agent.id)
            .await
            .unwrap();
        let known = snapshot.creation_provenance(&first.agent.id).unwrap();
        assert!(known.created_by.is_none());
        if trigger {
            assert!(matches!(
                known.creation_origin,
                AgentCreationOrigin::Trigger {
                    launching_actor: None,
                    ..
                }
            ));
        } else {
            assert_eq!(known.creation_origin, AgentCreationOrigin::Manual);
        }
        snapshot.first_log = None;
        assert_eq!(
            snapshot
                .creation_provenance(&first.agent.id)
                .unwrap()
                .creation_origin,
            AgentCreationOrigin::LegacyUnknown
        );
        snapshot.first_log = Some(first.initial_log.clone());
        snapshot.first_log.as_mut().unwrap().appended_at = "2026-09-06T00:00:00Z".into();
        assert!(matches!(
            snapshot.creation_provenance(&first.agent.id),
            Err(SandboxStoreError::Corrupt(_))
        ));
        assert!(!store
            .lock()
            .agent_creation_provenance
            .contains_key(&first.agent.id));
    }
}

#[tokio::test]
async fn provenance_rejects_forged_attribution_lineage_and_owner_kind() {
    let store = ready_store(3).await;
    let first = created(
        store
            .admit_agent(admission("provenance-corruption"))
            .await
            .unwrap(),
    );
    let mut snapshot = store
        .agent_provenance_snapshot(&first.agent.id)
        .await
        .unwrap();
    let original = snapshot.creation.clone().unwrap();
    for forged in [
        AgentCreationProvenance {
            created_by: Some(actor("")),
            ..original.clone()
        },
        AgentCreationProvenance {
            agent_created_at: "2026-09-06T00:00:00Z".into(),
            ..original.clone()
        },
        AgentCreationProvenance {
            owner: ResourceOwner::Organization {
                subject: SubjectId::new("owner"),
            },
            ..original.clone()
        },
        AgentCreationProvenance {
            creation_origin: AgentCreationOrigin::ChannelBridge {
                bridge_id: "forged".into(),
                partition_id: "forged".into(),
                generation: 1,
                source_reference: "forged".into(),
            },
            ..original.clone()
        },
    ] {
        snapshot.creation = Some(forged);
        assert!(matches!(
            snapshot.creation_provenance(&first.agent.id),
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
}

#[tokio::test]
async fn provenance_trigger_worker_does_not_invent_a_human_launcher() {
    let store = ready_store(3).await;
    let mut request = admission("provenance-trigger-worker");
    request.origin = AgentOrigin::Trigger {
        trigger_id: "original-trigger".into(),
        launch_id: "original-launch".into(),
    };
    request.created_by = Some(ResourceActor::Trigger {
        subject: SubjectId::new("launch-principal"),
        launch_id: crate::LaunchId("original-launch".into()),
    });
    let creation = created(store.admit_agent(request.clone()).await.unwrap());
    let record = store
        .agent_provenance_snapshot(&creation.agent.id)
        .await
        .unwrap()
        .creation_provenance(&creation.agent.id)
        .unwrap();
    assert_eq!(record.created_by, request.created_by);
    assert!(matches!(
        record.creation_origin,
        AgentCreationOrigin::Trigger {
            launching_actor: None,
            ..
        }
    ));
    request.creation_key = "different-launch".into();
    request.agent_id = AgentId::generate();
    request.worktree_id = WorktreeId::generate();
    request.created_by = Some(ResourceActor::Trigger {
        subject: SubjectId::new("launch-principal"),
        launch_id: crate::LaunchId("forged-launch".into()),
    });
    assert!(matches!(
        store.admit_agent(request).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert_eq!(store.lock().agent_creation_provenance.len(), 1);
}
