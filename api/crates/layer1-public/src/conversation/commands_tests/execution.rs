use super::fixtures::*;
use axum::{body::to_bytes, response::Response};
use layer2_core::{
    sandbox::{agent_io::*, store::*, *},
    AgentId,
};
use serde_json::Value;
use std::time::{Duration, SystemTime};

pub(super) async fn claim(f: &Fixture) -> AdmittedAgentCommand {
    let now = SystemTime::now();
    f.store
        .claim_next_command(CommandClaimRequest {
            agent_id: AgentId(ID.into()),
            holder: "protocol-worker".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .expect("queued command owns an execution lane")
}

pub(super) async fn execute(f: &Fixture) -> AdmittedAgentCommand {
    let claimed = claim(f).await;
    f.state
        .operations
        .execute_claimed_command(&context("owner-a").caller, &claimed)
        .await
        .unwrap();
    f.store
        .release_command_claim(
            &claimed.work.lease.as_ref().unwrap().claim,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    f.store
        .reconcile_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
}

pub(super) async fn commit(f: &Fixture, mut agent: Agent, logs: Vec<AgentLogKind>) {
    let current = f.store.get_agent(&agent.id).await.unwrap().unwrap();
    if agent.revision == current.revision {
        agent.revision += 1;
    }
    agent.conversation.last_seq += logs.len() as u64;
    f.store
        .commit_agent_conversation(AgentConversationMutation {
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: None,
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: 1,
            agent,
            appended_at: layer2_core::now_rfc3339(),
            agent_logs: logs,
        })
        .await
        .unwrap();
}

pub(super) async fn finish(f: &Fixture, text: &str) {
    let mut agent = f
        .store
        .get_agent(&AgentId(ID.into()))
        .await
        .unwrap()
        .unwrap();
    let turn = agent.conversation.active_turn.unwrap();
    let before = agent.status;
    let at = layer2_core::now_rfc3339();
    let mut logs = vec![];
    if let Some(operation) = agent.conversation.operation.as_mut() {
        if operation.state != AgentOperationDelivery::ExecutionObserved {
            operation.mark_execution_observed(at.clone()).unwrap();
            logs.push(AgentLogKind::OperationDeliveryChanged {
                operation_sequence: operation.sequence,
                operation: operation.intent.kind(),
                state: operation.state,
            });
        }
    }
    logs.extend([
        AgentLogKind::AgentMessage {
            content: text.into(),
        },
        AgentLogKind::TurnCompleted {
            turn_sequence: turn,
        },
        AgentLogKind::StatusChanged {
            from: before,
            to: AgentStatus::Ready,
        },
    ]);
    agent.conversation.active_turn = None;
    agent.transition(AgentStatus::Ready, at, None).unwrap();
    commit(f, agent, logs).await;
}

pub(super) async fn body(response: Response) -> String {
    let bytes = tokio::time::timeout(
        Duration::from_secs(2),
        to_bytes(response.into_body(), 1_048_576),
    )
    .await
    .unwrap()
    .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}
pub(super) fn events(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
