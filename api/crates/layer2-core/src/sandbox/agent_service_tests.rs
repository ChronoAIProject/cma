mod context;
mod control;
mod delegation;
mod fixtures;
mod lifecycle;
mod profile_provisioning;

use crate::profiles::store::AgentProfileStore;
use crate::profiles::{content_digest, AgentProfileRef, AgentProfileRevision, ProfileVisibility};
use crate::sandbox::service_trust::{validate_agent_profile, validate_agent_revision};
use crate::sandbox::store::{
    AgentLogStore, AgentStore, SandboxEventStore, SandboxStore, SandboxStoreError, WorktreeStore,
};
use crate::{AgentId, AgentProfileId, Capability, SubjectId};

use super::*;
use fixtures::*;

#[tokio::test]
async fn create_snapshots_profile_and_atomically_materializes_initial_records() {
    let (service, store, profiles, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        3,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let agent = service
        .create(
            &caller("owner", true),
            &workspace(WorkspaceStatus::Ready).id,
            request(reference.clone(), AgentOrigin::Manual),
        )
        .await
        .unwrap();

    assert_eq!(agent.status, AgentStatus::Starting);
    assert_eq!(
        agent.conversation.first_message.as_ref().unwrap().purpose,
        Some(FirstMessagePurpose::Task)
    );
    assert_eq!(agent.profile.id, reference.id);
    assert_eq!(agent.capabilities, agent_capabilities());
    assert_eq!(agent.conversation.last_seq, 1);
    assert_eq!(
        agent.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::Queued
    );
    assert_eq!(uses(&profiles, &reference.id).await, 1);
    let worktrees = store
        .list_worktrees(&agent.workspace_id, &Default::default())
        .await
        .unwrap()
        .items;
    assert_eq!(worktrees[0].name, "wt-1");
    assert_eq!(worktrees[0].agent_id.as_ref(), Some(&agent.id));
    assert!(matches!(
        store.agent_log(&agent.id, 0, 10).await.unwrap()[0].kind,
        AgentLogKind::Created { .. }
    ));
    assert_eq!(
        store
            .sandbox_events(&agent.sandbox_id, 0, 10)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn paused_agent_creation_admits_first_message_and_schedules_revival() {
    let (service, store, _, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        3,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let mut parent = store
        .get_sandbox(&workspace(WorkspaceStatus::Ready).sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let revision = parent.revision;
    parent.status = SandboxStatus::Paused;
    parent.runtime = None;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    parent.lifecycle.checkpoints.latest = Some(super::lifecycle_test_support::checkpoint(1, NOW));
    parent.revision += 1;
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    let agent = service
        .create(
            &caller("owner", true),
            &workspace(WorkspaceStatus::Ready).id,
            request(reference, AgentOrigin::Manual),
        )
        .await
        .unwrap();
    assert_eq!(agent.status, AgentStatus::Starting);
    assert_eq!(
        agent.conversation.first_message.as_ref().unwrap().state,
        FirstMessageDelivery::Queued
    );
    let reviving = store.get_sandbox(&parent.id).await.unwrap().unwrap();
    assert!(matches!(
        reviving.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            cause: SandboxReviveCause::AgentCreation,
            ..
        })
    ));
    assert!(!reviving.restores_agent(&agent));
}

#[tokio::test]
async fn create_refuses_a_runtime_without_conversation_and_records_nothing() {
    let (_, store, profiles, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        3,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let service = AgentService::new(
        store.clone(),
        profiles,
        TestAgentRuntimeRegistry::new(false),
        crate::RuntimeCapabilities::new([crate::RuntimeCapability::Resources]),
    );
    assert_eq!(
        service
            .create(
                &caller("owner", true),
                &workspace(WorkspaceStatus::Ready).id,
                request(reference, AgentOrigin::Manual),
            )
            .await,
        Err(AgentServiceError::CapabilityUnsupported {
            capability: crate::RuntimeCapability::Conversation,
        })
    );
    assert!(store
        .list_agents(&crate::SubjectId::new("owner"), &Default::default())
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn trigger_replay_is_exact_and_records_profile_use_once() {
    let (service, store, profiles, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        2,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let origin = AgentOrigin::Trigger {
        trigger_id: "trg_one".into(),
        launch_id: "launch-one".into(),
    };
    assert!(matches!(
        service
            .create(
                &caller("owner", true),
                &workspace(WorkspaceStatus::Ready).id,
                request(reference.clone(), origin.clone()),
            )
            .await,
        Err(AgentServiceError::InvalidRequest(_))
    ));
    assert_eq!(
        service
            .create_for_launch(
                &caller("owner", true),
                Capability::LaunchTriggers,
                &workspace(WorkspaceStatus::Ready).id,
                request(reference.clone(), origin.clone()),
            )
            .await,
        Err(AgentServiceError::Forbidden)
    );
    let owner = launch_caller("owner", Capability::CreateSandbox);
    assert_eq!(
        service
            .create_for_launch(
                &owner,
                Capability::UseOwnSandboxes,
                &workspace(WorkspaceStatus::Ready).id,
                request(reference.clone(), origin.clone()),
            )
            .await,
        Err(AgentServiceError::Forbidden)
    );
    assert!(matches!(
        service
            .create_for_launch(
                &owner,
                Capability::CreateSandbox,
                &workspace(WorkspaceStatus::Ready).id,
                request(reference.clone(), AgentOrigin::Manual),
            )
            .await,
        Err(AgentServiceError::InvalidRequest(_))
    ));
    assert_eq!(
        service
            .create_for_launch(
                &launch_caller("other", Capability::CreateSandbox),
                Capability::CreateSandbox,
                &workspace(WorkspaceStatus::Ready).id,
                request(reference.clone(), origin.clone()),
            )
            .await,
        Err(AgentServiceError::WorkspaceNotFound)
    );
    let first = service
        .create_for_launch(
            &owner,
            Capability::CreateSandbox,
            &workspace(WorkspaceStatus::Ready).id,
            request(reference.clone(), origin.clone()),
        )
        .await
        .unwrap();
    let replay = service
        .create_for_launch(
            &owner,
            Capability::CreateSandbox,
            &workspace(WorkspaceStatus::Ready).id,
            request(reference.clone(), origin.clone()),
        )
        .await
        .unwrap();
    use crate::sandbox::store::AgentProvenanceStore;
    let provenance = store
        .agent_provenance_snapshot(&first.id)
        .await
        .unwrap()
        .creation
        .unwrap();
    assert!(
        matches!(provenance.created_by, Some(crate::resource_access::ResourceActor::Trigger { subject, launch_id }) if subject == owner.subject && matches!(&origin, AgentOrigin::Trigger { launch_id: original, .. } if launch_id.as_str() == original))
    );
    assert!(matches!(
        provenance.creation_origin,
        crate::sandbox::AgentCreationOrigin::Trigger {
            launching_actor: None,
            ..
        }
    ));
    assert_eq!(replay.id, first.id);
    assert_eq!(uses(&profiles, &reference.id).await, 1);
    assert_eq!(
        store
            .list_agents(&owner.subject, &Default::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );

    let mut changed = request(reference, origin);
    changed.first_message = Some("A changed payload.".into());
    assert_eq!(
        service
            .create_for_launch(
                &owner,
                Capability::CreateSandbox,
                &workspace(WorkspaceStatus::Ready).id,
                changed,
            )
            .await,
        Err(AgentServiceError::CreationConflict)
    );
}

#[tokio::test]
async fn frozen_launch_revision_survives_profile_retirement() {
    let (service, _, profiles, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        2,
        "publisher",
        ProfileVisibility::Public,
    )
    .await;
    profiles
        .retire_agent_profile(&reference.id, &SubjectId::new("publisher"), NOW.into())
        .await
        .unwrap();

    assert_eq!(
        service
            .create(
                &caller("owner", true),
                &workspace(WorkspaceStatus::Ready).id,
                request(reference.clone(), AgentOrigin::Manual),
            )
            .await,
        Err(AgentServiceError::ProfileNotFound)
    );
    let visitor = launch_caller("owner", Capability::LaunchTriggers);
    let origin = AgentOrigin::Trigger {
        trigger_id: "trg_retired".into(),
        launch_id: "lch_retired".into(),
    };
    let created = service
        .create_for_launch(
            &visitor,
            Capability::LaunchTriggers,
            &workspace(WorkspaceStatus::Ready).id,
            request(reference.clone(), origin.clone()),
        )
        .await
        .unwrap();
    let replay = service
        .create_for_launch(
            &visitor,
            Capability::LaunchTriggers,
            &workspace(WorkspaceStatus::Ready).id,
            request(reference.clone(), origin),
        )
        .await
        .unwrap();
    assert_eq!(replay, created);
    assert_eq!(uses(&profiles, &reference.id).await, 1);
}

#[tokio::test]
async fn create_returns_typed_readiness_and_limit_refusals() {
    for (sandbox_status, workspace_status, expected) in [
        (
            SandboxStatus::Active,
            WorkspaceStatus::Preparing,
            AgentServiceError::WorkspaceNotReady,
        ),
        (
            SandboxStatus::Installing,
            WorkspaceStatus::Ready,
            AgentServiceError::SandboxNotReady,
        ),
    ] {
        let (service, _, _, reference) = setup(
            sandbox_status,
            workspace_status,
            1,
            "owner",
            ProfileVisibility::Private,
        )
        .await;
        assert_eq!(
            service
                .create(
                    &caller("owner", true),
                    &workspace(workspace_status).id,
                    request(reference, AgentOrigin::Manual),
                )
                .await,
            Err(expected)
        );
    }

    let (service, _, _, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        1,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let owner = caller("owner", true);
    service
        .create(
            &owner,
            &workspace(WorkspaceStatus::Ready).id,
            request(reference.clone(), AgentOrigin::Manual),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .create(
                &owner,
                &workspace(WorkspaceStatus::Ready).id,
                request(reference, AgentOrigin::Manual),
            )
            .await,
        Err(AgentServiceError::LimitReached { limit: 1, live: 1 })
    );
}

#[tokio::test]
async fn authorization_visibility_and_reads_fail_closed() {
    let (service, _, _, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        1,
        "publisher",
        ProfileVisibility::Private,
    )
    .await;
    assert_eq!(
        service
            .create(
                &caller("owner", false),
                &workspace(WorkspaceStatus::Ready).id,
                request(reference.clone(), AgentOrigin::Manual),
            )
            .await,
        Err(AgentServiceError::Forbidden)
    );
    assert_eq!(
        service
            .create(
                &caller("owner", true),
                &workspace(WorkspaceStatus::Ready).id,
                request(reference, AgentOrigin::Manual),
            )
            .await,
        Err(AgentServiceError::ProfileNotFound)
    );

    let (service, _, _, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        1,
        "publisher",
        ProfileVisibility::Public,
    )
    .await;
    let agent = service
        .create(
            &caller("owner", true),
            &workspace(WorkspaceStatus::Ready).id,
            request(reference, AgentOrigin::Manual),
        )
        .await
        .unwrap();
    assert_eq!(
        service.read(&caller("stranger", true), &agent.id).await,
        Err(AgentServiceError::NotFound)
    );
    assert_eq!(
        service
            .log(&caller("owner", true), &agent.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn request_bounds_and_profile_integrity_are_enforced() {
    let (service, store, profiles, stored_reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        1,
        "owner",
        ProfileVisibility::Private,
    )
    .await;
    let mut invalid = request(stored_reference.clone(), AgentOrigin::Manual);
    invalid.title = Some("x".repeat(65));
    assert_eq!(
        service
            .create(
                &caller("owner", true),
                &workspace(WorkspaceStatus::Ready).id,
                invalid,
            )
            .await
            .unwrap_err()
            .code(),
        "invalid_agent_request"
    );

    let reference = AgentProfileRef::new(AgentProfileId("agp_expected".into()), 2);
    let wrong_profile = profile("agp_wrong", "owner", ProfileVisibility::Private);
    assert!(matches!(
        validate_agent_profile(&reference, &wrong_profile),
        Err(crate::profiles::store::ProfileStoreError::Corrupt(_))
    ));
    let content = wrong_profile.draft;
    let mut revision = AgentProfileRevision {
        profile_id: reference.id.clone(),
        revision: reference.revision,
        digest: content_digest(&content).unwrap(),
        content,
        published_at: NOW.into(),
    };
    assert!(validate_agent_revision(&reference, &revision).is_ok());
    revision.digest = "0".repeat(64);
    assert!(matches!(
        validate_agent_revision(&reference, &revision),
        Err(crate::profiles::store::ProfileStoreError::Corrupt(_))
    ));

    for (index, corruption) in [
        AgentProfileCorruption::RevisionProfileId,
        AgentProfileCorruption::RevisionDigest,
    ]
    .into_iter()
    .enumerate()
    {
        let service = service_with_profile_corruption(store.clone(), profiles.clone(), corruption);
        let origin = AgentOrigin::Trigger {
            trigger_id: "trg_corrupt".into(),
            launch_id: format!("lch_corrupt_{index}"),
        };
        assert!(matches!(
            service
                .create_for_launch(
                    &launch_caller("owner", Capability::LaunchTriggers),
                    Capability::LaunchTriggers,
                    &workspace(WorkspaceStatus::Ready).id,
                    request(stored_reference.clone(), origin),
                )
                .await,
            Err(AgentServiceError::Corrupt(_))
        ));
    }
}

#[test]
fn service_error_codes_are_stable() {
    let values = [
        (AgentServiceError::Forbidden, "forbidden"),
        (
            AgentServiceError::ProfileNotFound,
            "agent_profile_not_found",
        ),
        (AgentServiceError::WorkspaceNotFound, "workspace_not_found"),
        (AgentServiceError::NotFound, "agent_not_found"),
        (AgentServiceError::WorkspaceNotReady, "workspace_not_ready"),
        (AgentServiceError::SandboxNotReady, "sandbox_not_ready"),
        (
            AgentServiceError::CapabilityUnsupported {
                capability: crate::RuntimeCapability::Conversation,
            },
            "agent_capability_unsupported",
        ),
        (
            AgentServiceError::LimitReached { limit: 1, live: 1 },
            "agent_limit_reached",
        ),
        (
            AgentServiceError::CreationConflict,
            "agent_creation_conflict",
        ),
        (
            AgentServiceError::StatusConflict {
                status: AgentStatus::Stopped,
            },
            "agent_status_conflict",
        ),
        (
            AgentServiceError::WorkConflict {
                step: AgentWorkStep::PublishStopped,
            },
            "agent_work_conflict",
        ),
        (
            AgentServiceError::StopReasonConflict,
            "agent_stop_reason_conflict",
        ),
        (AgentServiceError::ControlConflict, "agent_control_conflict"),
        (
            AgentServiceError::LogCapacityExceeded,
            "agent_log_capacity_exceeded",
        ),
        (
            AgentServiceError::InvalidRequest("bad".into()),
            "invalid_agent_request",
        ),
        (
            AgentServiceError::Corrupt("bad".into()),
            "stored_agent_corrupt",
        ),
        (
            AgentServiceError::Unavailable("down".into()),
            "agent_service_unavailable",
        ),
    ];
    for (error, code) in values {
        assert_eq!(error.code(), code);
    }
    assert_eq!(
        SandboxStoreError::AgentCreationConflict.to_string(),
        "agent creation key is already bound to another request"
    );
    assert!(AgentId::generate().as_str().starts_with("agt_"));
}
