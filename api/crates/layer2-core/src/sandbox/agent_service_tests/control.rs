use crate::profiles::ProfileVisibility;
use crate::sandbox::store::{AgentControlStoreError, AgentStore};
use crate::AgentId;

use super::super::*;
use super::fixtures::*;

async fn create_starting_agent() -> (
    std::sync::Arc<AgentService>,
    std::sync::Arc<crate::sandbox::store::InMemorySandboxStore>,
    Agent,
) {
    let (service, store, _, reference) = setup(
        SandboxStatus::Active,
        WorkspaceStatus::Ready,
        1,
        "owner",
        ProfileVisibility::Private,
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
    (service, store, agent)
}

#[tokio::test]
async fn agent_control_service_authorizes_before_lookup_and_hides_missing_or_foreign_agents() {
    let (service, _, agent) = create_starting_agent().await;
    let denied = caller("owner", false);
    let missing = AgentId("agt_missing".into());
    assert_eq!(
        service
            .stop(&denied, &missing, AgentStopReason::Owner)
            .await,
        Err(AgentServiceError::Forbidden)
    );
    assert_eq!(
        service.delete(&denied, &agent.id).await,
        Err(AgentServiceError::Forbidden)
    );

    let stranger = caller("stranger", true);
    assert_eq!(
        service
            .stop(&stranger, &agent.id, AgentStopReason::Owner)
            .await,
        Err(AgentServiceError::NotFound)
    );
    assert_eq!(
        service.delete(&stranger, &agent.id).await,
        Err(AgentServiceError::NotFound)
    );
    assert_eq!(
        service
            .stop(&caller("owner", true), &missing, AgentStopReason::Owner)
            .await,
        Err(AgentServiceError::NotFound)
    );
}

#[tokio::test]
async fn agent_control_service_stop_replays_only_until_stopped_and_does_not_overwrite_reason() {
    let (service, store, agent) = create_starting_agent().await;
    let owner = caller("owner", true);
    let scheduled = service
        .stop(&owner, &agent.id, AgentStopReason::Owner)
        .await
        .unwrap();
    assert_eq!(scheduled.status, AgentStatus::Starting);
    assert_eq!(scheduled.stopped_at, None);
    assert_eq!(scheduled.stop_reason, None);
    assert_eq!(
        scheduled.requested_stop_reason,
        Some(AgentStopReason::Owner)
    );
    assert_eq!(scheduled.work.step, AgentWorkStep::FenceInteraction);
    assert_eq!(
        service
            .stop(&owner, &agent.id, AgentStopReason::Owner)
            .await
            .unwrap(),
        scheduled
    );
    assert_eq!(
        service
            .stop(&owner, &agent.id, AgentStopReason::Operator)
            .await,
        Err(AgentServiceError::StopReasonConflict)
    );
    assert_eq!(
        service.delete(&owner, &agent.id).await,
        Err(AgentServiceError::StatusConflict {
            status: AgentStatus::Starting
        })
    );

    let mut stopped = scheduled.clone();
    stopped.work.schedule(AgentWorkStep::None, NOW.into());
    stopped
        .transition(
            AgentStatus::Stopped,
            NOW.into(),
            Some(AgentStopReason::Owner),
        )
        .unwrap();
    store
        .save_agent(scheduled.revision, stopped.clone())
        .await
        .unwrap();
    assert_eq!(
        service
            .stop(&owner, &agent.id, AgentStopReason::Owner)
            .await,
        Err(AgentServiceError::StatusConflict {
            status: AgentStatus::Stopped
        })
    );

    let deleting = service.delete(&owner, &stopped.id).await.unwrap();
    assert_eq!(deleting.status, AgentStatus::Stopped);
    assert_eq!(deleting.deleted_at, None);
    assert_eq!(deleting.work.step, AgentWorkStep::EnsureProcessClosed);
    assert_eq!(service.delete(&owner, &stopped.id).await.unwrap(), deleting);
}

#[tokio::test]
async fn agent_control_service_delete_requires_terminal_status_and_compatible_work() {
    let (service, store, starting) = create_starting_agent().await;
    let owner = caller("owner", true);
    assert_eq!(
        service.delete(&owner, &starting.id).await,
        Err(AgentServiceError::StatusConflict {
            status: AgentStatus::Starting
        })
    );

    let mut failed = starting.clone();
    failed
        .fail(
            TypedFailure {
                class: "startup_failed".into(),
                detail: "startup failed".into(),
            },
            NOW.into(),
        )
        .unwrap();
    failed.work.schedule(AgentWorkStep::None, NOW.into());
    store
        .save_agent(starting.revision, failed.clone())
        .await
        .unwrap();
    let deleting = service.delete(&owner, &failed.id).await.unwrap();
    assert_eq!(deleting.status, AgentStatus::Failed);
    assert_eq!(deleting.work.step, AgentWorkStep::EnsureProcessClosed);

    let (service, store, mut conflicted) = create_starting_agent().await;
    conflicted.status = AgentStatus::Stopped;
    conflicted.stopped_at = Some(NOW.into());
    conflicted.stop_reason = Some(AgentStopReason::Owner);
    conflicted.revision += 1;
    store
        .save_agent(conflicted.revision - 1, conflicted.clone())
        .await
        .unwrap();
    assert_eq!(
        service.delete(&caller("owner", true), &conflicted.id).await,
        Err(AgentServiceError::WorkConflict {
            step: AgentWorkStep::AddWorktree
        })
    );
}

#[test]
fn agent_control_service_error_codes_and_store_errors_are_stable() {
    let cases = [
        (
            AgentServiceError::StatusConflict {
                status: AgentStatus::Failed,
            },
            "agent_status_conflict",
        ),
        (
            AgentServiceError::WorkConflict {
                step: AgentWorkStep::RemoveHome,
            },
            "agent_work_conflict",
        ),
        (
            AgentServiceError::StopReasonConflict,
            "agent_stop_reason_conflict",
        ),
        (AgentServiceError::ControlConflict, "agent_control_conflict"),
    ];
    for (error, code) in cases {
        assert_eq!(error.code(), code);
    }
    assert_eq!(
        AgentControlStoreError::StatusConflict {
            status: AgentStatus::Stopped
        }
        .to_string(),
        "agent status does not permit the requested control operation: Stopped"
    );
}
