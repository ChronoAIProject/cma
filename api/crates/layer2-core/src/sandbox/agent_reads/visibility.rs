use crate::sandbox::{AgentLogEntry, AgentLogKind, AgentTranscript, AgentTurnOutcome};

/// Preserves the durable cursor even when a record is outside the reader's
/// scope. Redaction is a projection and never modifies the original log.
#[derive(Debug, Clone)]
pub struct AgentVisibleLogEntry {
    pub seq: u64,
    pub appended_at: String,
    pub kind: Option<AgentLogKind>,
}
impl AgentVisibleLogEntry {
    pub(super) fn from_entry(entry: AgentLogEntry, personal: bool) -> Self {
        let kind = if personal {
            Some(entry.kind)
        } else {
            shared_kind(entry.kind)
        };
        Self {
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
            failure.detail = "This Agent turn failed.".into();
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
            failure.detail = "This Agent turn failed.".into();
        }
    }
}
