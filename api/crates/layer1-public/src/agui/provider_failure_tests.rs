use super::*;
use layer2_core::sandbox::{
    AgentEventStreamSnapshot, AgentRetainedOutputAvailability, AgentVisibleLogEntry, TypedFailure,
};

#[tokio::test]
async fn provider_failure_follow_queue_preserves_cursor_and_retained_output() {
    for authorized in [false, true] {
        for available in [false, true] {
            let (mut open, agent) = following().await;
            let failure = if available {
                TypedFailure { class: "agent_log_capacity_exceeded".into(), detail: "This Agent cannot continue because its conversation history is full. Some partial output could not be saved in the conversation history. It is retained privately; contact support to recover it through an operator export.".into() }
            } else {
                TypedFailure {
                    class: "PRIVATE_CLASS_SENTINEL".into(),
                    detail: "unexpected status 503: PRIVATE_BODY_SENTINEL".into(),
                }
            };
            let entry = AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: 6,
                appended_at: "2026-11-19T00:00:00Z".into(),
                kind: AgentLogKind::TurnFailed {
                    turn_sequence: 3,
                    failure,
                },
            };
            if authorized {
                open.queue_authorized(AgentEventStreamSnapshot {
                    id: agent.id.clone(),
                    created_at: agent.created_at.clone(),
                    model: None,
                    initial_model: None,
                    runtime_generation: Some(1),
                    active_turn: None,
                    head: 6,
                    history: vec![AgentVisibleLogEntry::from_entry(entry, false)],
                });
            } else {
                open.queue(&[entry]);
            }
            let errors = open
                .pending
                .iter()
                .filter(|e| e.kind == "RUN_ERROR")
                .collect::<Vec<_>>();
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].seq, Some(6));
            assert_eq!(errors[0].timestamp.as_deref(), Some("2026-11-19T00:00:00Z"));
            assert_eq!(errors[0].run_id, "3");
            assert_eq!(
                errors[0].body.get("cma:retainedOutput").cloned(),
                available.then(|| serde_json::json!(AgentRetainedOutputAvailability {
                    available: true
                }))
            );
            assert!(!serde_json::to_string(&open.pending)
                .unwrap()
                .contains("SENTINEL"));
        }
    }
}
