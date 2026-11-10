use crate::sandbox::{AgentLogEntry, AgentLogKind, AgentTranscript, AgentTurnOutcome};

/// Preserves the durable cursor even when a record is outside the reader's
/// scope. Redaction is a projection and never modifies the original log.
#[derive(Debug, Clone)]
pub struct AgentVisibleLogEntry {
    /// Internal proof for response projection, never serialized into public history.
    pub deferred_failure_detail: Option<String>,
    pub seq: u64,
    pub appended_at: String,
    pub kind: Option<AgentLogKind>,
}
impl AgentVisibleLogEntry {
    pub(super) fn from_entry(entry: AgentLogEntry, personal: bool) -> Self {
        let deferred_failure_detail = match &entry.kind {
            AgentLogKind::TurnFailed { failure, .. }
                if failure.class == crate::sandbox::agent_io::SOURCE_DEFERRED_FAILURE =>
            {
                Some(failure.detail.clone())
            }
            _ => None,
        };
        let kind = if personal {
            Some(entry.kind)
        } else {
            shared_kind(entry.kind)
        };
        Self {
            deferred_failure_detail,
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
        TurnFailed {
            turn_sequence,
            mut failure,
        } => {
            redact_failure(&mut failure);
            Some(TurnFailed {
                turn_sequence,
                failure,
            })
        }
        Stopped { .. } => Some(Stopped {
            reason: "stopped".into(),
        }),
        kind @ (OwnerMessage { .. }
        | AgentMessage { .. }
        | TurnCompleted { .. }
        | TurnInterrupted { .. }
        | ToolCallStarted { .. }
        | ToolCallUpdated { .. }
        | ToolOutputReported { .. }
        | ApprovalResolved { .. }
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
        if let AgentTurnOutcome::Failed { failure, .. } = &mut turn.outcome {
            redact_failure(failure);
        }
    }
}

fn redact_failure(failure: &mut crate::sandbox::TypedFailure) {
    failure.detail = match failure.class.as_str() {
        "insufficient_credits" => "The model request failed with HTTP 402 because its billing account has insufficient credits. Ask the Sandbox owner to top up the account used for model requests, then send a new message to continue. This failed turn will not be replayed automatically.",
        _ => "This Agent turn failed.",
    }.into();
}

#[cfg(test)]
mod credit_failure_tests {
    use super::*;

    #[test]
    fn shared_credit_failure_exposes_static_guidance_and_never_private_diagnostics() {
        for class in ["insufficient_credits", "agent_turn_failed"] {
            let Some(AgentLogKind::TurnFailed {
                failure,
                turn_sequence,
            }) = shared_kind(AgentLogKind::TurnFailed {
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
                assert_eq!(failure.detail, "This Agent turn failed.");
            }
        }
    }
}
