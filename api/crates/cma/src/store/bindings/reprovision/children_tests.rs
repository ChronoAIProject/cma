use super::*;
use layer2_core::sandbox::{
    AgentCleanupDebt, AgentOperationFailure, AgentOperationIntent, AgentOperationRecord,
    AgentStatus, AgentStopReason, AgentWorkLease, QueuedAgentMessage,
};
use serde_json::json;

const AT: &str = "2026-10-29T08:13:00Z";

fn completed_agent() -> Agent {
    let mut operation = AgentOperationRecord::new(
        1,
        1,
        1,
        AgentOperationIntent::send_message("fixture message"),
        AT.into(),
    )
    .unwrap();
    operation.mark_execution_observed(AT.into()).unwrap();
    serde_json::from_value(json!({
        "id":"agt_fixture", "worktree_id":"wkt_fixture", "workspace_id":"wks_fixture",
        "sandbox_id":"sbx_fixture", "owner":"owner", "creation_key":"fixture",
        "creation_fingerprint":"a".repeat(64),
        "profile":{"id":"agp_fixture","revision":1,"digest":"b".repeat(64),"content":{
            "isolated_agent":false,"thinking_effort":"low","permission_level":"guarded",
            "skills":[],"system_prompt":"fixture"}},
        "status":"stopped", "conversation":{"last_seq":3,"operation":operation,"queued_messages":[]},
        "work":{"step":"none","ready_at":AT,"next_fence":1,"attempts":0,"receipts":[],"cleanup_debt":[]},
        "created_at":AT,"updated_at":AT,"stopped_at":AT,"stop_reason":"sandbox_retired",
        "origin":{"type":"manual"},"revision":1
    })).unwrap()
}

#[test]
fn terminal_agent_keeps_valid_observed_operation_without_changing_history() {
    let agent = completed_agent();
    let original = serde_json::to_vec(&agent).unwrap();
    assert!(settled_agent(&agent));
    assert_eq!(serde_json::to_vec(&agent).unwrap(), original);
    let mut without_operation = agent.clone();
    without_operation.conversation.operation = None;
    assert!(settled_agent(&without_operation));
    for status in [
        AgentStatus::Stopped,
        AgentStatus::Failed,
        AgentStatus::Deleted,
    ] {
        let mut terminal = agent.clone();
        terminal.status = status;
        assert!(settled_agent(&terminal), "rejected {status:?}");
    }
}

#[test]
fn preparation_rejects_unresolved_failed_or_malformed_operation_receipts() {
    for state in [
        AgentOperationDelivery::DispatchFenced,
        AgentOperationDelivery::WriteConfirmed,
        AgentOperationDelivery::PreWriteFailed,
        AgentOperationDelivery::RecoveryRequired,
        AgentOperationDelivery::RuntimeLost,
    ] {
        let mut agent = completed_agent();
        let mut operation = AgentOperationRecord::new(
            1,
            1,
            1,
            AgentOperationIntent::send_message("fixture message"),
            AT.into(),
        )
        .unwrap();
        let failure = || AgentOperationFailure::new("failed", "fixture", AT).unwrap();
        match state {
            AgentOperationDelivery::DispatchFenced => {}
            AgentOperationDelivery::WriteConfirmed => {
                operation.mark_write_confirmed(AT.into()).unwrap()
            }
            AgentOperationDelivery::PreWriteFailed => {
                operation.mark_pre_write_failed(failure()).unwrap()
            }
            AgentOperationDelivery::RecoveryRequired => {
                operation.require_recovery(failure()).unwrap()
            }
            AgentOperationDelivery::RuntimeLost => {
                operation.retire_lost_runtime(failure()).unwrap()
            }
            AgentOperationDelivery::ExecutionObserved => unreachable!(),
        }
        assert!(operation.validate().is_ok());
        agent.conversation.operation = Some(operation);
        assert!(!settled_agent(&agent), "accepted {state:?}");
    }
    for field in [
        "missing_timestamp",
        "invalid_timestamp",
        "failed_at",
        "failure",
        "sequence",
        "intent",
    ] {
        let mut agent = completed_agent();
        let operation = agent.conversation.operation.as_mut().unwrap();
        match field {
            "missing_timestamp" => operation.execution_observed_at = None,
            "invalid_timestamp" => operation.execution_observed_at = Some("not-a-date".into()),
            "failed_at" => operation.failed_at = Some(AT.into()),
            "failure" => {
                operation.failure =
                    Some(AgentOperationFailure::new("failed", "fixture", AT).unwrap())
            }
            "sequence" => operation.sequence = 0,
            "intent" => {
                operation.intent = AgentOperationIntent::SendMessage {
                    payload_sha256: "invalid".into(),
                }
            }
            _ => unreachable!(),
        }
        assert!(!settled_agent(&agent), "accepted {field}");
    }
}

#[test]
fn observed_operation_does_not_bypass_live_work_or_stop_intent() {
    for field in ["live", "step", "lease", "debt", "stop", "turn", "queue"] {
        let mut agent = completed_agent();
        match field {
            "live" => agent.status = AgentStatus::Ready,
            "step" => agent.work.step = AgentWorkStep::CloseProcess,
            "lease" => {
                agent.work.lease = Some(AgentWorkLease {
                    holder: "worker".into(),
                    fence: 1,
                    expires_at: AT.into(),
                })
            }
            "debt" => agent.work.cleanup_debt.push(AgentCleanupDebt {
                step: AgentWorkStep::RemoveHome,
                operation_id: "cleanup".into(),
                request_sha256: "c".repeat(64),
            }),
            "stop" => agent.requested_stop_reason = Some(AgentStopReason::Owner),
            "turn" => agent.conversation.active_turn = Some(1),
            "queue" => agent
                .conversation
                .queued_messages
                .push(QueuedAgentMessage::new(1, "queued".into(), AT.into()).unwrap()),
            _ => unreachable!(),
        }
        assert!(!settled_agent(&agent), "accepted {field}");
    }
}
