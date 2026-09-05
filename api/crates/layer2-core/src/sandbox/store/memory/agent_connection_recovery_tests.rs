use std::sync::Arc;

use crate::sandbox::store::{
    AgentAdmissionStore, AgentConnectionRecoveryCause, AgentConnectionRecoveryOutcome,
    AgentConnectionRecoveryRequest, AgentStore, AgentWorkerStore, PageRequest,
};
use crate::sandbox::{Agent, AgentStatus, AgentStopReason, AgentWorkStep};

use super::agent_admission_tests::{admission, created, ready_store, NOW};
use super::InMemorySandboxStore;

const LATER: &str = "2026-09-05T00:01:00Z";

async fn insert_live(store: &InMemorySandboxStore, key: &str, status: AgentStatus) -> Agent {
    let creation = created(store.admit_agent(admission(key)).await.unwrap());
    let mut agent = creation.agent;
    agent.status = status;
    agent.conversation.conversation_id = Some(format!("conversation-{key}"));
    agent.conversation.runtime_generation = Some(7);
    agent.conversation.active_turn =
        matches!(status, AgentStatus::Working | AgentStatus::WaitingApproval).then_some(11);
    agent.ready_at = Some(NOW.into());
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    agent.work.next_fence = 13;
    agent.revision = 4;
    agent.validate_conversation().unwrap();
    store.lock().agents.insert(agent.id.clone(), agent.clone());
    agent
}

fn request(agent: &Agent) -> AgentConnectionRecoveryRequest {
    AgentConnectionRecoveryRequest {
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
        expected_revision: agent.revision,
        expected_work_step: agent.work.step,
        expected_authority_fence: Some(12),
        requested_at: LATER.into(),
        cause: AgentConnectionRecoveryCause::UpdateStreamLost,
    }
}

#[tokio::test]
async fn live_connection_scan_is_complete_content_free_and_paginated() {
    let store = ready_store(8).await;
    let ready = insert_live(&store, "b-ready", AgentStatus::Ready).await;
    let working = insert_live(&store, "a-working", AgentStatus::Working).await;
    let paused = insert_live(&store, "c-paused", AgentStatus::Paused).await;
    let missing = created(store.admit_agent(admission("no-identity")).await.unwrap()).agent;

    let first = store
        .live_agent_connections(&PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert!(first.next.is_some());
    let second = store
        .live_agent_connections(&PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    let mut ids: Vec<_> = first
        .items
        .into_iter()
        .chain(second.items)
        .map(|candidate| candidate.agent_id)
        .collect();
    ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    let mut expected = vec![ready.id, working.id, paused.id];
    expected.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    assert_eq!(ids, expected);
    assert!(!ids.contains(&missing.id));
}

#[tokio::test]
async fn recovery_schedules_every_attached_live_status_without_changing_conversation_state() {
    for status in [
        AgentStatus::Ready,
        AgentStatus::Working,
        AgentStatus::WaitingApproval,
    ] {
        let store = ready_store(1).await;
        let original = insert_live(&store, status.name(), status).await;
        let original_conversation = original.conversation.clone();
        assert_eq!(
            store
                .schedule_agent_connection_recovery(request(&original))
                .await
                .unwrap(),
            AgentConnectionRecoveryOutcome::Scheduled
        );
        let scheduled = store.get_agent(&original.id).await.unwrap().unwrap();
        assert_eq!(scheduled.status, status);
        assert_eq!(scheduled.conversation, original_conversation);
        assert_eq!(scheduled.work.step, AgentWorkStep::PublishConnection);
        assert_eq!(scheduled.work.next_fence, original.work.next_fence);
        assert_eq!(scheduled.revision, original.revision + 1);
        assert_eq!(scheduled.updated_at, LATER);
    }
}

#[tokio::test]
async fn recovery_is_idempotent_and_concurrent_requests_mutate_once() {
    let store = Arc::new(ready_store(1).await);
    let agent = insert_live(store.as_ref(), "concurrent", AgentStatus::Ready).await;
    let left = store.schedule_agent_connection_recovery(request(&agent));
    let right = store.schedule_agent_connection_recovery(request(&agent));
    let (left, right) = tokio::join!(left, right);
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == AgentConnectionRecoveryOutcome::Scheduled)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == AgentConnectionRecoveryOutcome::AlreadyScheduled)
            .count(),
        1
    );
    assert_eq!(
        store.get_agent(&agent.id).await.unwrap().unwrap().revision,
        agent.revision + 1
    );
}

#[tokio::test]
async fn stale_identity_revision_step_and_authority_are_superseded() {
    let store = ready_store(1).await;
    let agent = insert_live(&store, "stale", AgentStatus::Working).await;

    let mut wrong_identity = request(&agent);
    wrong_identity.conversation_id = "replacement-conversation".into();
    assert_eq!(
        store
            .schedule_agent_connection_recovery(wrong_identity)
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::Superseded
    );
    let mut stale_revision = request(&agent);
    stale_revision.expected_revision -= 1;
    assert_eq!(
        store
            .schedule_agent_connection_recovery(stale_revision)
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::Superseded
    );
    let mut stale_step = request(&agent);
    stale_step.expected_work_step = AgentWorkStep::DispatchFirstMessage;
    assert_eq!(
        store
            .schedule_agent_connection_recovery(stale_step)
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::Superseded
    );
    let mut stale_authority = request(&agent);
    stale_authority.expected_authority_fence = Some(agent.work.next_fence);
    assert_eq!(
        store
            .schedule_agent_connection_recovery(stale_authority)
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::Superseded
    );
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn recovery_cause_requires_the_matching_attachment_fence_shape() {
    let store = ready_store(1).await;
    let agent = insert_live(&store, "cause-shape", AgentStatus::Ready).await;
    let mut eof_without_fence = request(&agent);
    eof_without_fence.expected_authority_fence = None;
    assert!(matches!(
        store
            .schedule_agent_connection_recovery(eof_without_fence)
            .await,
        Err(crate::sandbox::store::SandboxStoreError::InvalidTransaction(_))
    ));

    let mut restart_with_fence = request(&agent);
    restart_with_fence.cause = AgentConnectionRecoveryCause::ControlPlaneRestart;
    assert!(matches!(
        store
            .schedule_agent_connection_recovery(restart_with_fence)
            .await,
        Err(crate::sandbox::store::SandboxStoreError::InvalidTransaction(_))
    ));
    assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
}

#[tokio::test]
async fn stop_intent_paused_state_and_other_outboxes_win_over_recovery() {
    let store = ready_store(3).await;
    let mut stopping = insert_live(&store, "stopping", AgentStatus::Ready).await;
    stopping.requested_stop_reason = Some(AgentStopReason::Owner);
    stopping
        .work
        .schedule(AgentWorkStep::FenceInteraction, NOW.into());
    stopping.revision += 1;
    store
        .lock()
        .agents
        .insert(stopping.id.clone(), stopping.clone());
    let mut stopping_request = request(&stopping);
    stopping_request.expected_work_step = AgentWorkStep::FenceInteraction;
    assert_eq!(
        store
            .schedule_agent_connection_recovery(stopping_request)
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::NotEligible
    );

    let paused = insert_live(&store, "paused", AgentStatus::Paused).await;
    assert_eq!(
        store
            .schedule_agent_connection_recovery(request(&paused))
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::NotEligible
    );

    let mut dispatching = insert_live(&store, "dispatching", AgentStatus::Ready).await;
    dispatching
        .work
        .schedule(AgentWorkStep::DispatchFirstMessage, NOW.into());
    store
        .lock()
        .agents
        .insert(dispatching.id.clone(), dispatching.clone());
    assert_eq!(
        store
            .schedule_agent_connection_recovery(request(&dispatching))
            .await
            .unwrap(),
        AgentConnectionRecoveryOutcome::NotEligible
    );
    assert_eq!(
        store
            .get_agent(&dispatching.id)
            .await
            .unwrap()
            .unwrap()
            .work
            .step,
        AgentWorkStep::DispatchFirstMessage
    );
}
