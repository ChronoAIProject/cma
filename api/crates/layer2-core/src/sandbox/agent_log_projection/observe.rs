use super::*;
use AgentLogEvidenceChange as Change;

impl AgentLogProjectionState {
    pub(super) fn apply(
        &mut self,
        agent: &Agent,
        entry: &AgentLogEntry,
        queued_owner: Option<u64>,
    ) -> Result<Vec<Change>, String> {
        let mut changes = Vec::new();
        if matches!(
            entry.kind,
            AgentLogKind::TurnStarted { .. }
                | AgentLogKind::TurnCompleted { .. }
                | AgentLogKind::TurnFailed { .. }
                | AgentLogKind::TurnInterrupted { .. }
                | AgentLogKind::Stopped { .. }
                | AgentLogKind::Deleted
        ) {
            self.approval_epoch = entry.seq;
            self.latest_plan = None;
        }
        if entry.seq == 1 && !matches!(entry.kind, AgentLogKind::Created { .. }) {
            return Err("Agent log must begin with creation".into());
        }
        if super::super::AgentTaskStatus::relevant(&entry.kind) {
            self.latest_task_status = Some(entry.seq);
        }
        match &entry.kind {
            AgentLogKind::AvailableCommandsSet { .. } => self.latest_commands = Some(entry.seq),
            AgentLogKind::RuntimeMetadataChanged { .. } => self.latest_metadata = Some(entry.seq),
            AgentLogKind::PlanSet { .. } if self.active.is_some() => {
                self.latest_plan = Some(entry.seq)
            }
            AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } => {
                if entry.seq != 1
                    || workspace_id != &agent.workspace_id
                    || worktree_id != agent.creation_worktree_id()
                {
                    return Err("Agent creation lineage is invalid".into());
                }
            }
            AgentLogKind::ConversationOpened {
                conversation_id,
                runtime_generation,
            } => {
                if self.active.is_some()
                    || conversation_id.trim().is_empty()
                    || conversation_id.len() > 512
                    || *runtime_generation == 0
                    || self
                        .latest_conversation
                        .as_ref()
                        .is_some_and(|(_, n)| runtime_generation <= n)
                {
                    return Err("Agent conversation boundary is invalid".into());
                }
                self.latest_conversation = Some((conversation_id.clone(), *runtime_generation));
                self.usage.open_generation(*runtime_generation)?;
            }
            AgentLogKind::RuntimeGenerationRetired {
                previous_generation,
                runtime_generation,
            } => {
                self.latest_commands = None;
                let Some((_, generation)) = self.latest_conversation.as_mut() else {
                    return Err("Agent retirement has no native identity".into());
                };
                if self.active.is_some()
                    || generation != previous_generation
                    || previous_generation.checked_add(1) != Some(*runtime_generation)
                {
                    return Err("Agent retirement boundary is invalid".into());
                }
                *generation = *runtime_generation;
            }
            AgentLogKind::WorkStepCompleted {
                step: super::super::AgentWorkStep::OpenConversation,
            } => {
                if self.open_completion.is_some() {
                    self.duplicate_open_completion = true;
                } else {
                    self.open_completion = Some((entry.seq, entry.appended_at.clone()));
                }
            }
            AgentLogKind::TurnStarted {
                turn_sequence,
                origin_key,
                model,
            } => {
                if *turn_sequence != entry.seq
                    || self.active.is_some()
                    || self.latest_conversation.is_none()
                    || model.as_ref().is_some_and(|model| {
                        model.len() > 256
                            || model.trim() != model
                            || model.chars().any(char::is_control)
                    })
                {
                    return Err("Agent turn start is invalid".into());
                }
                let setup =
                    agent.conversation.first_message.as_ref().filter(|first| {
                        first.purpose != Some(super::super::FirstMessagePurpose::Task)
                    });
                if self.title_turn.is_none()
                    && setup.is_none_or(|first| {
                        first.turn_sequence.is_some_and(|seq| *turn_sequence > seq)
                    })
                {
                    self.title_turn = Some(*turn_sequence);
                }
                let title_eligible =
                    queued_owner.is_none() && (setup.is_some() || !self.has_owner_input);
                let previous_turn = self.latest_turn;
                self.latest_turn = Some(*turn_sequence);
                self.active = Some(AgentLogTurnFacts {
                    turn_sequence: *turn_sequence,
                    previous_turn,
                    started_at: entry.appended_at.clone(),
                    origin_key: origin_key.clone(),
                    owner_sequence: queued_owner,
                    first_assistant_sequence: None,
                    last_message: queued_owner.map(|seq| (seq, AgentTranscriptRole::Owner, true)),
                    metadata_sequence: None,
                    lost_frames: 0,
                    first_gap_sequence: None,
                    end_sequence: None,
                    source_completed: false,
                    title_eligible,
                    outcome: AgentTurnOutcome::Active,
                });
                if let Some(sequence) = queued_owner {
                    self.unconsumed_queue
                        .retain(|reference| reference.sequence != sequence);
                    changes.push(Change::Queued {
                        sequence,
                        consumed_by: Some(*turn_sequence),
                    });
                    changes.push(Change::Message {
                        sequence,
                        turn_sequence: *turn_sequence,
                        role: AgentTranscriptRole::Owner,
                    });
                }
            }
            AgentLogKind::TurnMetadataSet {
                turn_sequence,
                metadata,
            } => {
                let turn = self
                    .active
                    .as_mut()
                    .filter(|t| t.turn_sequence == *turn_sequence && t.metadata_sequence.is_none())
                    .ok_or("Agent turn metadata has no unassigned active turn")?;
                if metadata.is_empty()
                    || metadata.len() > 16
                    || metadata
                        .iter()
                        .any(|(k, v)| k.chars().count() > 64 || v.chars().count() > 512)
                {
                    return Err("Agent turn metadata is invalid".into());
                }
                turn.metadata_sequence = Some(entry.seq);
            }
            AgentLogKind::OwnerMessage { content } => {
                self.has_owner_input = true;
                if let Some(turn) = &mut self.active {
                    let steering = self.steering.take().is_some();
                    if steering {
                        turn.title_eligible = false;
                    }
                    if !steering && turn.owner_sequence.is_some() {
                        return Err("Agent owner message is duplicated".into());
                    }
                    if !steering {
                        turn.owner_sequence = Some(entry.seq);
                    }
                    turn.last_message = Some((
                        entry.seq,
                        AgentTranscriptRole::Owner,
                        !content.trim().is_empty(),
                    ));
                    changes.push(Change::Message {
                        sequence: entry.seq,
                        turn_sequence: turn.turn_sequence,
                        role: AgentTranscriptRole::Owner,
                    });
                } else {
                    if self.unconsumed_queue.len() >= super::super::MAX_QUEUED_AGENT_MESSAGES {
                        return Err("Agent queued input exceeds its bounded capacity".into());
                    }
                    self.unconsumed_queue.push(AgentLogQueuedReference {
                        sequence: entry.seq,
                        payload_sha256: legacy::payload_digest(content),
                    });
                    changes.push(Change::Queued {
                        sequence: entry.seq,
                        consumed_by: None,
                    });
                }
            }
            AgentLogKind::AgentMessage { content } => {
                let turn = self
                    .active
                    .as_mut()
                    .filter(|t| t.owner_sequence.is_some())
                    .ok_or("Agent message has no active owner input")?;
                turn.first_assistant_sequence.get_or_insert(entry.seq);
                turn.last_message = Some((
                    entry.seq,
                    AgentTranscriptRole::Agent,
                    !content.trim().is_empty(),
                ));
                changes.push(Change::Message {
                    sequence: entry.seq,
                    turn_sequence: turn.turn_sequence,
                    role: AgentTranscriptRole::Agent,
                });
            }
            AgentLogKind::TurnCompleted { turn_sequence }
            | AgentLogKind::TurnFailed { turn_sequence, .. }
            | AgentLogKind::TurnInterrupted { turn_sequence } => {
                let mut turn = self
                    .active
                    .take()
                    .filter(|t| t.turn_sequence == *turn_sequence && t.owner_sequence.is_some())
                    .ok_or("Agent terminal turn does not match")?;
                turn.end_sequence = Some(entry.seq);
                turn.source_completed = matches!(entry.kind, AgentLogKind::TurnCompleted { .. });
                turn.outcome = match &entry.kind {
                    AgentLogKind::TurnCompleted { .. } if turn.lost_frames == 0 => AgentTurnOutcome::Completed { at: entry.appended_at.clone() },
                    AgentLogKind::TurnFailed { failure, .. } => AgentTurnOutcome::Failed { at: entry.appended_at.clone(), failure: failure.clone() },
                    AgentLogKind::TurnInterrupted { .. } => AgentTurnOutcome::Interrupted { at: entry.appended_at.clone() },
                    _ => AgentTurnOutcome::Failed { at: entry.appended_at.clone(), failure: super::super::TypedFailure {
                        class: "conversation_output_gap".into(), detail: format!("{} runtime output frames were lost. Recorded messages are preserved, but this output is incomplete.", turn.lost_frames),
                    } },
                };
                changes.push(Change::Turn(turn));
            }
            AgentLogKind::OperationDeliveryChanged {
                operation_sequence,
                operation,
                state,
            } => {
                if *operation == AgentOperationKind::Steer
                    && *state == AgentOperationDelivery::DispatchFenced
                {
                    if *operation_sequence != entry.seq
                        || self
                            .active
                            .as_ref()
                            .is_none_or(|t| t.owner_sequence.is_none())
                    {
                        return Err("Agent steering fence has no active owner input".into());
                    }
                    self.steering = Some((entry.seq, entry.appended_at.clone()));
                }
                changes.push(Change::Operation {
                    operation_sequence: *operation_sequence,
                    sequence: entry.seq,
                });
            }
            AgentLogKind::ApprovalRequested { approval_id, .. } => changes.push(Change::Approval {
                approval_id: approval_id.clone(),
                request_sequence: Some(entry.seq),
                turn_sequence: self.active.as_ref().map(|t| t.turn_sequence),
                epoch: self.approval_epoch,
            }),
            AgentLogKind::ApprovalResolved { approval_id, .. } => changes.push(Change::Approval {
                approval_id: approval_id.clone(),
                request_sequence: None,
                turn_sequence: self.active.as_ref().map(|t| t.turn_sequence),
                epoch: self.approval_epoch,
            }),
            AgentLogKind::Usage {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
            } => self.usage.observe(
                *input_tokens,
                *output_tokens,
                *context_used,
                *context_window,
            )?,
            AgentLogKind::UsageUpdated {
                input_tokens,
                output_tokens,
                context_used,
                context_window,
                turn_sequence,
                turn_input_tokens,
                turn_output_tokens,
            } => {
                self.usage.observed = Some(AgentUsage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    context_used: *context_used,
                    context_window: *context_window,
                    current_turn: turn_sequence.map(|turn_sequence| super::super::AgentTurnUsage {
                        turn_sequence,
                        input_tokens: *turn_input_tokens,
                        output_tokens: *turn_output_tokens,
                    }),
                });
            }
            AgentLogKind::ConversationGap { frames } => {
                if self.latest_conversation.is_none() || *frames == 0 {
                    return Err("Agent conversation gap is invalid".into());
                }
                if let Some(turn) = &mut self.active {
                    turn.first_gap_sequence.get_or_insert(entry.seq);
                    turn.lost_frames = turn
                        .lost_frames
                        .checked_add(*frames)
                        .ok_or("Agent gap count overflowed")?;
                }
            }
            _ => {}
        }
        if let Some(turn) = &self.active {
            changes.push(Change::Turn(turn.clone()));
        }
        Ok(changes)
    }
}
