//! Apply only this append batch to the persisted working state.
use super::*;
use crate::sandbox::queued_message::queued_message_origin_sequence;

impl AgentLogCheckpoint {
    pub(super) fn apply_entry(&mut self, entry: &AgentLogEntry) -> Result<(), SandboxStoreError> {
        use crate::sandbox::{AgentLogKind, AgentOperationDelivery, AgentOperationKind};
        if seconds_from_rfc3339(&entry.appended_at).is_none() {
            return Err(SandboxStoreError::Corrupt(
                "agent log timestamp is invalid".into(),
            ));
        }
        if self
            .steering_expected_sequence
            .is_some_and(|expected| expected != entry.seq)
        {
            return Err(SandboxStoreError::Corrupt(
                "steering fence has no adjacent owner message".into(),
            ));
        }
        match &entry.kind {
            AgentLogKind::Created { .. } => {
                if self.creation_seen || entry.seq != 1 {
                    return Err(SandboxStoreError::Corrupt(
                        "agent log contains duplicate creation".into(),
                    ));
                }
                self.creation_seen = true;
            }
            AgentLogKind::ConversationOpened {
                conversation_id,
                runtime_generation,
            } => {
                if self.active_turn_sequence.is_some()
                    || conversation_id.trim().is_empty()
                    || conversation_id.len() > 512
                    || *runtime_generation == 0
                    || self
                        .runtime_generation
                        .is_some_and(|previous| *runtime_generation <= previous)
                {
                    return Err(SandboxStoreError::Corrupt(
                        "invalid conversation boundary".into(),
                    ));
                }
                self.conversation_id = Some(conversation_id.clone());
                self.runtime_generation = Some(*runtime_generation);
            }
            AgentLogKind::RuntimeGenerationRetired {
                previous_generation,
                runtime_generation,
            } => {
                if self.active_turn_sequence.is_some()
                    || self.runtime_generation != Some(*previous_generation)
                    || previous_generation.checked_add(1) != Some(*runtime_generation)
                {
                    return Err(SandboxStoreError::Corrupt(
                        "invalid runtime generation transition".into(),
                    ));
                }
                self.runtime_generation = Some(*runtime_generation);
            }
            AgentLogKind::TurnStarted {
                turn_sequence,
                origin_key,
                model,
                ..
            } => {
                if self.conversation_id.is_none()
                    || self.active_turn_sequence.is_some()
                    || *turn_sequence == 0
                    || *turn_sequence != entry.seq
                    || self
                        .latest_started_turn_sequence
                        .is_some_and(|previous| previous >= *turn_sequence)
                    || model.as_ref().is_some_and(|model| {
                        model.len() > 256
                            || model.trim() != model
                            || model.chars().any(char::is_control)
                    })
                {
                    return Err(SandboxStoreError::Corrupt("nested or invalid turn".into()));
                }
                self.latest_started_turn_sequence = Some(*turn_sequence);
                self.active_turn_sequence = Some(*turn_sequence);
                self.latest_message_sequence = None;
                self.latest_message_role = None;
                self.latest_message_nonempty = false;
                self.active_owner_message = false;
                self.active_agent_message_count = 0;
                self.active_gap_frames = 0;
                self.turn_metadata_set = false;
                self.latest_outcome = Some(TurnOutcome::Active);
                self.answer_turn_sequence = None;
                self.answer_sequence = None;
                if let Some(sequence) = origin_key
                    .as_deref()
                    .and_then(queued_message_origin_sequence)
                {
                    let queued = self
                        .queued_owner_messages
                        .iter()
                        .find(|message| message.sequence == sequence)
                        .cloned()
                        .ok_or_else(|| {
                            SandboxStoreError::Corrupt("queued turn has no owner message".into())
                        })?;
                    self.active_owner_message = true;
                    self.latest_message_sequence = Some(queued.sequence);
                    self.latest_message_role = Some(MessageRole::Owner);
                    self.latest_message_nonempty = queued.nonempty;
                    self.queued_owner_messages
                        .retain(|message| message.sequence != sequence);
                }
            }
            AgentLogKind::TurnMetadataSet { turn_sequence, .. } => {
                if self.active_turn_sequence != Some(*turn_sequence) || self.turn_metadata_set {
                    return Err(SandboxStoreError::Corrupt(
                        "invalid turn metadata transition".into(),
                    ));
                }
                let AgentLogKind::TurnMetadataSet { metadata, .. } = &entry.kind else {
                    unreachable!()
                };
                if metadata.is_empty()
                    || metadata.len() > 16
                    || metadata
                        .iter()
                        .any(|(key, value)| key.chars().count() > 64 || value.chars().count() > 512)
                {
                    return Err(SandboxStoreError::Corrupt("invalid turn metadata".into()));
                }
                self.turn_metadata_set = true;
            }
            AgentLogKind::OwnerMessage { content } => {
                let steering = self.steering_expected_sequence == Some(entry.seq);
                if self.steering_expected_sequence.is_some() && !steering {
                    return Err(SandboxStoreError::Corrupt(
                        "steering fence has no owner message".into(),
                    ));
                }
                if steering
                    && self.steering_fence_appended_at.as_deref()
                        != Some(entry.appended_at.as_str())
                {
                    return Err(SandboxStoreError::Corrupt(
                        "steering owner message timestamp disagrees with its fence".into(),
                    ));
                }
                self.steering_expected_sequence = None;
                self.steering_fence_appended_at = None;
                if self.active_turn_sequence.is_none() {
                    if self
                        .queued_owner_messages
                        .iter()
                        .any(|message| message.sequence == entry.seq)
                    {
                        return Err(SandboxStoreError::Corrupt(
                            "duplicate queued owner message".into(),
                        ));
                    }
                    self.queued_owner_messages.push(QueuedOwnerMessage {
                        sequence: entry.seq,
                        content_sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
                        nonempty: !content.trim().is_empty(),
                    });
                } else {
                    if !steering
                        && (self.active_owner_message || self.active_agent_message_count > 0)
                    {
                        return Err(SandboxStoreError::Corrupt(
                            "invalid owner message transition".into(),
                        ));
                    }
                    if !steering {
                        self.active_owner_message = true;
                    }
                    self.latest_message_sequence = Some(entry.seq);
                    self.latest_message_role = Some(MessageRole::Owner);
                    self.latest_message_nonempty = !content.trim().is_empty();
                }
            }
            AgentLogKind::AgentMessage { content } => {
                if self.active_turn_sequence.is_none()
                    || !self.active_owner_message
                    || self.steering_expected_sequence.is_some()
                {
                    return Err(SandboxStoreError::Corrupt(
                        "agent message has no owner turn".into(),
                    ));
                }
                self.active_agent_message_count = self.active_agent_message_count.saturating_add(1);
                self.latest_message_sequence = Some(entry.seq);
                self.latest_message_role = Some(MessageRole::Agent);
                self.latest_message_nonempty = !content.trim().is_empty();
            }
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                operation: AgentOperationKind::Steer,
                state: AgentOperationDelivery::DispatchFenced,
            } if *operation_sequence == entry.seq && self.active_owner_message => {
                self.steering_expected_sequence = Some(
                    entry
                        .seq
                        .checked_add(1)
                        .ok_or(SandboxStoreError::SequenceConflict)?,
                );
                self.steering_fence_appended_at = Some(entry.appended_at.clone());
            }
            AgentLogKind::OperationDeliveryChanged {
                operation: AgentOperationKind::Steer,
                state: AgentOperationDelivery::DispatchFenced,
                ..
            } => {
                return Err(SandboxStoreError::Corrupt(
                    "invalid steering dispatch fence".into(),
                ));
            }
            AgentLogKind::OperationDeliveryChanged { .. } => {}
            AgentLogKind::TurnCompleted { turn_sequence } => {
                self.finish_turn(*turn_sequence, TurnOutcome::Completed)?;
            }
            AgentLogKind::TurnFailed { turn_sequence, .. } => {
                self.finish_turn(*turn_sequence, TurnOutcome::Failed)?;
            }
            AgentLogKind::TurnInterrupted { turn_sequence } => {
                self.finish_turn(*turn_sequence, TurnOutcome::Interrupted)?;
            }
            AgentLogKind::ConversationGap { frames } => {
                if self.conversation_id.is_none() || *frames == 0 {
                    return Err(SandboxStoreError::Corrupt(
                        "invalid conversation gap".into(),
                    ));
                }
                if self.active_turn_sequence.is_some() {
                    self.active_gap_frames =
                        self.active_gap_frames.checked_add(*frames).ok_or_else(|| {
                            SandboxStoreError::Corrupt(
                                "agent conversation gap count overflowed".into(),
                            )
                        })?;
                }
            }
            AgentLogKind::Usage { .. } if self.conversation_id.is_none() => {
                return Err(SandboxStoreError::Corrupt(
                    "agent usage precedes its conversation".into(),
                ));
            }
            AgentLogKind::SkillInstallationChanged { .. }
            | AgentLogKind::WorkStepCompleted { .. }
            | AgentLogKind::WorkStepFailed { .. }
            | AgentLogKind::ToolCallStarted { .. }
            | AgentLogKind::ToolCallUpdated { .. }
            | AgentLogKind::ToolOutputReported { .. }
            | AgentLogKind::ApprovalRequested { .. }
            | AgentLogKind::ApprovalResolved { .. }
            | AgentLogKind::ExternalApprovalRequested { .. }
            | AgentLogKind::ExternalApprovalResolved { .. }
            | AgentLogKind::Usage { .. }
            | AgentLogKind::RuntimeMetadataChanged { .. }
            | AgentLogKind::UsageUpdated { .. }
            | AgentLogKind::ModeChanged { .. }
            | AgentLogKind::PlanSet { .. }
            | AgentLogKind::ConfigOptionsSet { .. }
            | AgentLogKind::AvailableCommandsSet { .. }
            | AgentLogKind::SignInRequired { .. }
            | AgentLogKind::StatusChanged { .. }
            | AgentLogKind::FirstMessageChanged { .. }
            | AgentLogKind::TitleSet { .. }
            | AgentLogKind::Stopped { .. }
            | AgentLogKind::Deleted => {}
        }
        Ok(())
    }

    fn finish_turn(
        &mut self,
        sequence: u64,
        outcome: TurnOutcome,
    ) -> Result<(), SandboxStoreError> {
        if self.active_turn_sequence != Some(sequence) || !self.active_owner_message {
            return Err(SandboxStoreError::Corrupt(
                "turn outcome does not match an owner turn".into(),
            ));
        }
        let publish = matches!(outcome, TurnOutcome::Completed)
            && self.active_gap_frames == 0
            && self.latest_message_role == Some(MessageRole::Agent)
            && self.latest_message_nonempty;
        self.latest_outcome = Some(
            if matches!(outcome, TurnOutcome::Completed) && self.active_gap_frames > 0 {
                TurnOutcome::Failed
            } else {
                outcome
            },
        );
        self.answer_turn_sequence = publish.then_some(sequence);
        self.answer_sequence = publish.then_some(self.latest_message_sequence.unwrap_or_default());
        self.active_turn_sequence = None;
        self.latest_message_sequence = None;
        self.latest_message_role = None;
        self.latest_message_nonempty = false;
        self.active_owner_message = false;
        self.active_agent_message_count = 0;
        self.active_gap_frames = 0;
        self.steering_expected_sequence = None;
        self.steering_fence_appended_at = None;
        self.turn_metadata_set = false;
        Ok(())
    }
}
