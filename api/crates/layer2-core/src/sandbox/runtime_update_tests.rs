use super::lifecycle_test_support::{agent, NOW};
use super::runtime_update_test_support::native;
use super::store::{
    apply_image_rotation, SandboxImageRotationRequest, SandboxMaintenanceApplication,
    SandboxMaintenanceOutcome,
};
use super::*;

#[test]
fn runtime_update_deleted_history_is_idle_only_after_cleanup_and_dispatch_finish() {
    let mut deleted = agent("deleted", "preserved-conversation", 9);
    deleted.conversation.active_turn = Some(2);
    deleted
        .transition(
            AgentStatus::Stopped,
            NOW.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    deleted
        .transition(AgentStatus::Deleted, NOW.into(), None)
        .unwrap();
    assert!(runtime_update_agents_are_idle(std::slice::from_ref(
        &deleted
    )));
    for variant in 0..9 {
        let mut blocked = deleted.clone();
        match variant {
            0 => blocked.deleted_at = None,
            1 => blocked.deleted_at = Some("invalid".into()),
            2 => blocked.status = AgentStatus::Stopped,
            3 => blocked.work.step = AgentWorkStep::RemoveHome,
            4 => {
                blocked.work.lease = Some(AgentWorkLease {
                    holder: "cleanup".into(),
                    fence: 1,
                    expires_at: NOW.into(),
                })
            }
            5 => blocked.work.cleanup_debt.push(AgentCleanupDebt {
                step: AgentWorkStep::OpenConversation,
                operation_id: "pending-cleanup".into(),
                request_sha256: "a".repeat(64),
            }),
            6 => blocked
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(3, "pending".into(), NOW.into()).unwrap()),
            7 => blocked.requested_stop_reason = Some(AgentStopReason::Owner),
            _ => {
                blocked.conversation.operation = Some(
                    AgentOperationRecord::new(3, 4, 2, AgentOperationIntent::Interrupt, NOW.into())
                        .unwrap(),
                )
            }
        }
        assert!(
            !runtime_update_agents_are_idle(&[blocked]),
            "variant {variant}"
        );
    }
    // The original attribution and interrupted turn are retained as history.
    assert_eq!(deleted.stop_reason, Some(AgentStopReason::Owner));
    assert_eq!(deleted.conversation.active_turn, Some(2));
}

#[test]
fn runtime_update_daily_boundary_is_0200_singapore_and_survives_restart() {
    let mut value = native();
    assert!(value
        .check_daily_runtime_update("image-v2", "2026-10-19T06:00:00Z")
        .unwrap());
    assert!(
        value.runtime_update_request().is_none(),
        "first deployment does not rotate at14:00SGT"
    );
    assert!(!value
        .check_daily_runtime_update("image-v2", "2026-10-19T17:59:59Z")
        .unwrap());
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut value: Sandbox = serde_json::from_slice(&bytes).unwrap();
    assert!(value
        .check_daily_runtime_update("image-v2", "2026-10-19T18:00:00Z")
        .unwrap());
    assert!(value.runtime_update_request().unwrap().automatic);
    assert_eq!(
        value.runtime_update_request().unwrap().target_version,
        "image-v2"
    );
    let revision = value.revision;
    assert!(!value
        .check_daily_runtime_update("image-v2", "2026-10-20T06:00:00Z")
        .unwrap());
    assert_eq!(value.revision, revision);
}

#[test]
fn runtime_update_new_deployment_waits_next_boundary_but_catches_up_same_target() {
    let mut value = native();
    value
        .check_daily_runtime_update("image-v2", "2026-10-18T06:00:00Z")
        .unwrap();
    let mut existing = value.clone();
    existing
        .check_daily_runtime_update("image-v2", "2026-10-19T06:00:00Z")
        .unwrap();
    assert!(
        existing.runtime_update_request().is_some(),
        "restart catches missed02:00 only for already observed target"
    );
    value
        .check_daily_runtime_update("image-v3", "2026-10-19T06:00:00Z")
        .unwrap();
    assert!(
        value.runtime_update_request().is_none(),
        "new deployment must not inherit old target deadline"
    );
    value
        .check_daily_runtime_update("image-v3", "2026-10-19T18:00:00Z")
        .unwrap();
    assert_eq!(
        value.runtime_update_request().unwrap().target_version,
        "image-v3"
    );
}

#[test]
fn runtime_update_queue_is_idempotent_and_never_retargets_pending_work() {
    let mut value = native();
    let before = value.clone();
    assert!(value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap());
    assert_eq!(value.runtime, before.runtime);
    assert_eq!(value.lifecycle.home, before.lifecycle.home);
    assert_eq!(value.work, before.work);
    assert_eq!(value.status, SandboxStatus::Active);
    let queued = value.clone();
    assert!(!value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap());
    assert_eq!(value, queued);
    assert!(value
        .request_runtime_update("image-v3".into(), false, NOW.into())
        .is_err());
    assert_eq!(value, queued);
    value
        .check_daily_runtime_update("image-v3", "2026-10-19T06:00:00Z")
        .unwrap();
    assert_eq!(
        value.runtime_update_request().unwrap().target_version,
        "image-v2"
    );
}

#[test]
fn runtime_update_excludes_custom_images_and_unretained_homes() {
    let mut value = native();
    value.profile.content.runtime_image = Some("custom-image".into());
    assert!(!value.supports_retained_runtime_update());
    assert!(value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .is_err());
    assert!(!value.check_daily_runtime_update("image-v2", NOW).unwrap());
    let mut value = super::lifecycle_test_support::sandbox();
    assert!(value
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .is_err());
}

#[test]
fn runtime_update_defers_busy_agent_and_admits_complete_idle_identity() {
    let mut sandbox = native();
    sandbox
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap();
    let request = SandboxImageRotationRequest {
        sandbox_id: sandbox.id.clone(),
        requested_at: NOW.into(),
        expected_runtime_version: "image-v2".into(),
    };
    let idle = agent("agent-id", "original-conversation", 1);
    for status in [AgentStatus::Starting, AgentStatus::Working] {
        let mut busy = idle.clone();
        busy.status = status;
        assert_eq!(
            apply_image_rotation(&sandbox, &[busy], &request).unwrap(),
            SandboxMaintenanceApplication::Complete(SandboxMaintenanceOutcome::NotEligible)
        );
    }
    let mut busy = idle.clone();
    busy.conversation.active_turn = Some(2);
    assert!(!runtime_update_agents_are_idle(&[busy]));
    let mut busy = idle.clone();
    busy.conversation
        .queued_messages
        .push(QueuedAgentMessage::new(2, "keep my message".into(), NOW.into()).unwrap());
    assert!(!runtime_update_agents_are_idle(&[busy]));
    let mut busy = idle.clone();
    busy.status = AgentStatus::Stopped;
    busy.work.step = AgentWorkStep::RemoveHome;
    assert!(!runtime_update_agents_are_idle(&[busy]));
    let mut busy = idle.clone();
    busy.work.step = AgentWorkStep::CloseProcess;
    assert!(!runtime_update_agents_are_idle(&[busy]));
    let SandboxMaintenanceApplication::Persist { sandbox: next, .. } =
        apply_image_rotation(&sandbox, std::slice::from_ref(&idle), &request).unwrap()
    else {
        panic!("idle update not admitted");
    };
    assert_eq!(next.status, SandboxStatus::Rotating);
    assert_eq!(next.runtime, sandbox.runtime);
    assert_eq!(next.lifecycle.home, sandbox.lifecycle.home);
    let SandboxLifecycleOperation::ImageRotation { identity, .. } =
        next.lifecycle.operation.unwrap()
    else {
        panic!()
    };
    assert!(identity.preserved_by(&sandbox, &[idle]));
}

#[tokio::test]
async fn runtime_update_service_conceals_foreign_owner_and_rejects_stale_target() {
    use super::store::{InMemorySandboxStore, SandboxStore};
    use crate::{Authorization, Caller, Capability, SubjectId};
    use std::sync::Arc;
    let store = Arc::new(InMemorySandboxStore::default());
    let mut value = native();
    value.revision = 1;
    store.create_sandbox(value.clone()).await.unwrap();
    let service = SandboxRuntimeUpdateService::new(store.clone(), "image-v2".into());
    let owner = Caller {
        subject: value.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let foreign = Caller {
        subject: SubjectId::new("foreign"),
        authorization: owner.authorization.clone(),
    };
    assert!(matches!(
        service.request(&foreign, &value.id, "image-v3").await,
        Err(SandboxRuntimeUpdateError::NotFound)
    ));
    assert!(matches!(
        service.request(&owner, &value.id, "image-v3").await,
        Err(SandboxRuntimeUpdateError::TargetChanged)
    ));
    let saved = service
        .request(&owner, &value.id, "image-v2")
        .await
        .unwrap();
    assert_eq!(
        service
            .request(&owner, &value.id, "image-v2")
            .await
            .unwrap(),
        saved
    );
    assert_eq!(saved.runtime, value.runtime);
    assert_eq!(saved.lifecycle.home, value.lifecycle.home);
}

#[tokio::test]
async fn runtime_update_operator_queues_foreign_owner_without_changing_live_identity_or_work() {
    use super::store::{InMemorySandboxStore, SandboxStore};
    use crate::{Authorization, Caller, Capability, SubjectId};
    use std::sync::Arc;
    let store = Arc::new(InMemorySandboxStore::default());
    let mut value = native();
    value.revision = 1;
    store.create_sandbox(value.clone()).await.unwrap();
    let service = SandboxRuntimeUpdateService::new(store.clone(), "image-v2".into());
    let caller = Caller {
        subject: SubjectId::new("actual-operator"),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    assert!(matches!(
        service
            .request_as_operator(&caller, &value.id, "image-v2")
            .await,
        Err(SandboxRuntimeUpdateError::Forbidden)
    ));
    let caller = Caller {
        authorization: Authorization::new([
            Capability::UseOwnSandboxes,
            Capability::ControlAnySandbox,
        ]),
        ..caller
    };
    assert!(
        matches!(
            service.request(&caller, &value.id, "image-v2").await,
            Err(SandboxRuntimeUpdateError::NotFound)
        ),
        "owner endpoint must still conceal foreign owners"
    );
    assert!(matches!(
        service
            .request_as_operator(&caller, &value.id, "image-v3")
            .await,
        Err(SandboxRuntimeUpdateError::TargetChanged)
    ));
    assert_eq!(
        store.get_sandbox(&value.id).await.unwrap(),
        Some(value.clone())
    );
    let queued = service
        .request_as_operator(&caller, &value.id, "image-v2")
        .await
        .unwrap();
    assert_eq!(queued.owner, value.owner);
    assert_eq!(queued.runtime, value.runtime);
    assert_eq!(queued.status, SandboxStatus::Active);
    assert_eq!(queued.lifecycle.home, value.lifecycle.home);
    assert_eq!(queued.lifecycle.operation, value.lifecycle.operation);
    assert_eq!(queued.work, value.work);
    let request = queued.runtime_update_request().unwrap();
    assert_eq!(request.target_version, "image-v2");
    assert!(request.started_at.is_none());
    assert!(request.completed_at.is_none());
    assert_eq!(
        service
            .request_as_operator(&caller, &value.id, "image-v2")
            .await
            .unwrap(),
        queued
    );
    let service = SandboxRuntimeUpdateService::new(store, "image-v3".into());
    assert_eq!(
        service
            .request_as_operator(&caller, &value.id, "image-v2")
            .await
            .unwrap(),
        queued,
        "already queued target remains retryable after a deployment"
    );
    assert!(
        matches!(
            service
                .request_as_operator(&caller, &value.id, "image-v3")
                .await,
            Err(SandboxRuntimeUpdateError::NotAvailable)
        ),
        "pending update cannot be retargeted"
    );
}
