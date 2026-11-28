use crate::sandbox::{
    public_agent_turn_failure, sanitize_agent_transcript, AgentLogEntry, AgentLogKind,
    AgentTranscript,
};

/// Preserves the durable cursor even when a record is outside the reader's
/// scope. Redaction is a projection and never modifies the original log.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentVisibleLogEntry {
    /// Internal proof for response projection, never serialized into public history.
    pub deferred_failure_detail: Option<String>,
    pub retained_output: Option<super::AgentRetainedOutputAvailability>,
    pub seq: u64,
    pub appended_at: String,
    pub kind: Option<AgentLogKind>,
}
impl AgentVisibleLogEntry {
    pub fn from_entry(entry: AgentLogEntry, personal: bool) -> Self {
        let deferred_failure_detail = match &entry.kind {
            AgentLogKind::TurnFailed { failure, .. }
                if failure.class == crate::sandbox::agent_io::SOURCE_DEFERRED_FAILURE =>
            {
                Some(failure.detail.clone())
            }
            _ => None,
        };
        let retained_output = match &entry.kind {
            AgentLogKind::TurnFailed { failure, .. } => {
                super::AgentRetainedOutputAvailability::from_failure(failure)
            }
            _ => None,
        };
        let mut kind = if personal {
            Some(entry.kind)
        } else {
            shared_kind(entry.kind)
        };
        if let Some(AgentLogKind::TurnFailed { failure, .. }) = &mut kind {
            *failure = public_agent_turn_failure(failure);
        }
        Self {
            deferred_failure_detail,
            retained_output,
            seq: entry.seq,
            appended_at: entry.appended_at,
            kind,
        }
    }
}

fn shared_kind(kind: AgentLogKind) -> Option<AgentLogKind> {
    use AgentLogKind::*;
    match kind {
        Created { .. }
        | ConversationOpened { .. }
        | RuntimeGenerationRetired { .. }
        | SkillInstallationChanged { .. }
        | WorkStepCompleted { .. }
        | WorkStepFailed { .. }
        | TurnMetadataSet { .. }
        | SignInRequired { .. }
        | FirstMessageChanged { .. }
        | RuntimeMetadataChanged { .. }
        | ConfigOptionsSet { .. }
        | AvailableCommandsSet { .. } => None,
        TurnStarted {
            turn_sequence,
            model,
            ..
        } => Some(TurnStarted {
            turn_sequence,
            model,
            origin_key: None,
        }),
        ApprovalRequested {
            approval_id,
            action,
            detail,
            ..
        } => Some(ApprovalRequested {
            approval_id,
            action,
            detail,
            runtime_locator: None,
        }),
        Stopped { .. } => Some(Stopped {
            reason: "stopped".into(),
        }),
        kind @ (OwnerMessage { .. }
        | AgentMessage { .. }
        | AgentMessageDelta { .. }
        | TurnCompleted { .. }
        | TurnFailed { .. }
        | TurnInterrupted { .. }
        | ToolCallStarted { .. }
        | ToolCallUpdated { .. }
        | ToolOutputReported { .. }
        | ApprovalResolved { .. }
        | ExternalApprovalRequested { .. }
        | ExternalApprovalResolved { .. }
        | OperationDeliveryChanged { .. }
        | Usage { .. }
        | UsageUpdated { .. }
        | ModeChanged { .. }
        | PlanSet { .. }
        | ConversationGap { .. }
        | StatusChanged { .. }
        | TitleSet { .. }
        | Deleted) => Some(kind),
    }
}

pub(super) fn shared_transcript(transcript: &mut AgentTranscript) {
    for turn in &mut transcript.turns {
        turn.origin_key = None;
        turn.metadata.clear();
    }
    sanitize_agent_transcript(transcript);
}

#[cfg(test)]
#[path = "retained_output_visibility_tests.rs"]
mod retained_output_tests;

#[cfg(test)]
mod credit_failure_tests {
    use super::*;

    fn visible_failure(kind: AgentLogKind) -> Option<AgentLogKind> {
        AgentVisibleLogEntry::from_entry(
            AgentLogEntry {
                agent_id: crate::AgentId("agent".into()),
                seq: 8,
                appended_at: "2026-11-19T00:00:00Z".into(),
                kind,
            },
            false,
        )
        .kind
    }

    #[test]
    fn shared_credit_failure_exposes_static_guidance_and_never_private_diagnostics() {
        for class in ["insufficient_credits", "agent_turn_failed"] {
            let Some(AgentLogKind::TurnFailed {
                failure,
                turn_sequence,
            }) = visible_failure(AgentLogKind::TurnFailed {
                turn_sequence: 7,
                failure: crate::sandbox::TypedFailure {
                    class: class.into(),
                    detail: "private billing owner and upstream diagnostics".into(),
                },
            })
            else {
                panic!("terminal failure must remain visible")
            };
            assert_eq!(turn_sequence, 7);
            assert_eq!(failure.class, class);
            assert!(!failure.detail.contains("private"));
            if class == "insufficient_credits" {
                assert!(failure.detail.contains("HTTP 402"));
                assert!(failure.detail.contains("top up"));
                assert!(failure.detail.contains("not be replayed automatically"));
            } else {
                assert!(failure.detail.starts_with("The Agent run failed."));
            }
        }
    }
    #[test]
    fn shared_budget_failures_expose_their_static_guidance() {
        use crate::budget::BudgetError;
        for reason in [
            BudgetError::OwnerExhausted,
            BudgetError::TurnExhausted,
            BudgetError::Unavailable,
            BudgetError::PlatformExhausted,
        ] {
            let Some(AgentLogKind::TurnFailed { failure, .. }) =
                visible_failure(AgentLogKind::TurnFailed {
                    turn_sequence: 3,
                    failure: crate::sandbox::TypedFailure {
                        class: reason.code().into(),
                        detail: "private settlement diagnostics".into(),
                    },
                })
            else {
                panic!("terminal failure must remain visible")
            };
            assert_eq!(failure.detail, reason.guidance(), "{}", reason.code());
        }
    }
}
