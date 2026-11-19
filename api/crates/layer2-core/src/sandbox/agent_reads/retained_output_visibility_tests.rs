use super::*;
use crate::sandbox::{
    AgentRetainedOutputAvailability, AgentTranscriptTurn, AgentTurnOutcome, TypedFailure,
};

#[test]
fn private_prefix_discovery_precedes_redaction_without_changing_failure_bytes() {
    let notice = "This Agent cannot continue because its conversation history is full. Some partial output could not be saved in the conversation history. It is retained privately; contact support to recover it through an operator export.";
    for (class, detail, available) in [
        ("agent_log_capacity_exceeded", notice.to_owned(), true),
        ("agent_log_capacity_exceeded", format!("{notice} private diagnostics"), false),
        ("agent_log_capacity_exceeded", "Durable Agent history reached its lifetime budget. Retained output and runtime identity are preserved. This Agent cannot continue under the current limits; inspect capacity before explicit recovery or replacement.".into(), false),
        ("agent_log_capacity_exceeded", "private diagnostics".into(), false),
        ("other_failure", notice.to_owned(), false),
    ] {
        let original = TypedFailure { class: class.into(), detail };
        let outcome = AgentTurnOutcome::Failed { at: "2026-11-17T00:00:00Z".into(), failure: original.clone() };
        let expected = available.then_some(AgentRetainedOutputAvailability { available: true });
        for personal in [false, true] {
            let visible = AgentVisibleLogEntry::from_entry(AgentLogEntry {
                agent_id: crate::ids::AgentId("agent".into()),
                seq: 8,
                appended_at: "2026-11-17T00:00:00Z".into(),
                kind: AgentLogKind::TurnFailed { turn_sequence: 7, failure: original.clone() },
            }, personal);
            assert_eq!(visible.retained_output, expected);
            let Some(AgentLogKind::TurnFailed { turn_sequence, failure }) = visible.kind else { panic!("visible failure") };
            assert_eq!(turn_sequence, 7);
            assert_eq!(failure, public_agent_turn_failure(&original));
            assert_ne!(failure.detail, original.detail);
        }
        let mut transcript = AgentTranscript {
            agent_id: crate::ids::AgentId("agent".into()),
            through_seq: 8,
            turns: vec![AgentTranscriptTurn {
                retained_output: AgentRetainedOutputAvailability::from_outcome(&outcome),
                turn_sequence: 7,
                started_at: "2026-11-17T00:00:00Z".into(),
                origin_key: Some("private-origin".into()),
                owner_message: "Question".into(),
                agent_messages: vec![],
                messages: vec![],
                metadata: [("private".into(), "diagnostics".into())].into(),
                outcome,
            }],
        };
        shared_transcript(&mut transcript);
        let turn = &transcript.turns[0];
        assert_eq!(turn.retained_output, expected);
        assert!(turn.origin_key.is_none() && turn.metadata.is_empty());
        assert!(matches!(&turn.outcome, AgentTurnOutcome::Failed { failure, .. } if *failure == public_agent_turn_failure(&original)));
    }
    assert!(AgentRetainedOutputAvailability::from_outcome(&AgentTurnOutcome::Active).is_none());
    assert!(
        AgentRetainedOutputAvailability::from_outcome(&AgentTurnOutcome::Completed {
            at: "now".into()
        })
        .is_none()
    );
}
