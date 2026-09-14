use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::sandbox::store::{
    AgentControlStore, AgentControlTarget, AgentStopRequest, AgentStore, AgentWorkMutation,
    AgentWorkerStore, InMemorySandboxStore,
};
use layer2_core::sandbox::{
    AgentLogKind, AgentOperationDelivery, AgentStatus, AgentStopReason, AgentWorkStep,
    SandboxEventKind,
};
use layer2_core::AgentId;

pub(super) async fn after_acknowledgement(
    store: Arc<InMemorySandboxStore>,
    id: AgentId,
    reason: AgentStopReason,
) {
    let agent = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let agent = store.get_agent(&id).await.unwrap().unwrap();
            if agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| operation.state == AgentOperationDelivery::WriteConfirmed)
            {
                break agent;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("runtime dispatch must acknowledge before stop scheduling");
    store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&agent),
            reason: reason.clone(),
            requested_at: layer2_core::now_rfc3339(),
        })
        .await
        .unwrap();

    // Match the worker's legal fenced cleanup path; the seed has no adapter receipts.
    for next in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishStopped,
        AgentWorkStep::None,
    ] {
        let now = SystemTime::now();
        let at = layer2_core::rfc3339_at(now);
        let mut agent = store
            .claim_agent_work(
                &id,
                "contract-stop-worker",
                at.clone(),
                layer2_core::rfc3339_at(now + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .expect("scheduled stop work is claimable");
        let expected_agent_revision = agent.revision;
        let fence = agent.work.lease.as_ref().unwrap().fence;
        let current = agent.work.step;
        agent
            .work
            .advance("contract-stop-worker", fence, next, at.clone())
            .unwrap();
        let (log, sandbox_event) = if next == AgentWorkStep::None {
            let from = agent.status;
            agent
                .transition(AgentStatus::Stopped, at.clone(), Some(reason.clone()))
                .unwrap();
            (
                AgentLogKind::Stopped {
                    reason: reason.as_str().into(),
                },
                Some(SandboxEventKind::AgentStatusChanged {
                    agent_id: id.clone(),
                    from,
                    to: AgentStatus::Stopped,
                }),
            )
        } else {
            agent.revision += 1;
            agent.updated_at = at.clone();
            (AgentLogKind::WorkStepCompleted { step: current }, None)
        };
        agent.conversation.last_seq += 1;
        store
            .commit_agent_work(AgentWorkMutation {
                expected_agent_revision,
                holder: "contract-stop-worker".into(),
                fence,
                agent,
                worktree: None,
                appended_at: at,
                sandbox_event,
                agent_logs: vec![log],
            })
            .await
            .unwrap();
    }
}
