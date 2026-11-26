use layer3_adapter::{ConversationUpdate, SessionUpdate};

use super::conversation_fold_support::{
    active_turn, append_bounded, approval_detail, bounded_ref as bounded, complete_first_message,
    fail_first_message, flush_message, observe_first_message, observe_operation, prompt_evidence,
    recorded_initial_tool_output, recorded_tool_output, validate_prompt_identity, wire_name,
};
use super::{
    Agent, AgentAvailableCommand, AgentConfigOption, AgentConversationError, AgentLogKind,
    AgentMessageAssembly, AgentPlanStep, AgentStatus, MAX_DURABLE_AGENT_MESSAGE_BYTES,
};

pub(super) struct ConversationChange {
    pub(super) agent: Agent,
    pub(super) logs: Vec<AgentLogKind>,
    pub(super) assembly: AgentMessageAssembly,
    pub(super) turn_closed: bool,
}

pub(super) enum FoldedConversationUpdate {
    Quiet(AgentMessageAssembly),
    Changed(Box<ConversationChange>),
}

pub(super) fn fold_update(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    update: impl std::borrow::Borrow<ConversationUpdate>,
    appended_at: &str,
    durable_tool_output: bool,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    if !current.status.is_live() {
        return Err(AgentConversationError::InvalidState(
            "conversation updates require a live agent",
        ));
    }
    let (update, prompt_evidence) = prompt_evidence(update.borrow());
    let timeout = current.conversation.turn_timeout.as_ref();
    if timeout.is_some_and(|timeout| timeout.blocks_dispatch()) {
        // Preserve the real source receipt in the recorder, but late output
        // cannot reopen a turn ended by service policy.
        return Ok(FoldedConversationUpdate::Quiet(assembly.clone()));
    }
    if let ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }
    | ConversationUpdate::FailedForPrompt { prompt_id, .. } = update
    {
        validate_prompt_identity(current, prompt_id)?;
    }
    if !prompt_evidence
        && current.conversation.active_turn.is_none()
        && update.turn_evidence().is_some()
    {
        // Restoring an idle conversation can emit its previous turn's history.
        // Persist the runtime receipt without duplicating that history or
        // inventing a turn. Recovered active turns still consume their output,
        // and conversation metadata remains valid while the Agent is idle.
        return Ok(FoldedConversationUpdate::Quiet(assembly.clone()));
    }
    match update {
        ConversationUpdate::Metadata(metadata) => {
            super::conversation_observation_fold::metadata(current, assembly, metadata, appended_at)
        }
        ConversationUpdate::Usage(usage) => {
            super::conversation_observation_fold::usage(current, assembly, usage, appended_at)
        }
        ConversationUpdate::Said(SessionUpdate::SessionInfoUpdate(info)) => {
            let description = info
                .title
                .value()
                .filter(|title| title.len() <= 4096)
                .cloned();
            super::conversation_observation_fold::metadata(
                current,
                assembly,
                &layer3_adapter::ConversationMetadata {
                    description,
                    working_directory: None,
                    ..Default::default()
                },
                appended_at,
            )
        }
        ConversationUpdate::Said(SessionUpdate::AgentMessageChunk(chunk)) => {
            fold_message_chunk(current, assembly, chunk, prompt_evidence, appended_at)
        }
        ConversationUpdate::Said(SessionUpdate::AgentThoughtChunk(_))
        | ConversationUpdate::Said(SessionUpdate::UserMessageChunk(_)) => {
            Ok(FoldedConversationUpdate::Quiet(assembly.clone()))
        }
        ConversationUpdate::Said(update) => fold_semantic_update(
            current,
            assembly,
            update,
            prompt_evidence,
            appended_at,
            durable_tool_output,
        ),
        ConversationUpdate::Asked(request) => {
            let turn = active_turn(current)?;
            if !matches!(
                current.status,
                AgentStatus::Working | AgentStatus::WaitingApproval
            ) {
                return Err(AgentConversationError::InvalidState(
                    "approval request requires a working agent",
                ));
            }
            let mut agent = current.clone();
            let mut logs = flush_message(assembly);
            observe_first_message(&mut agent, prompt_evidence, appended_at, &mut logs)?;
            observe_operation(&mut agent, prompt_evidence, false, appended_at, &mut logs)?;
            logs.push(AgentLogKind::ApprovalRequested {
                approval_id: request.id.clone(),
                action: request.request.tool_call.fields.title.clone(),
                detail: approval_detail(&request.request.options),
                runtime_locator: request
                    .locator
                    .as_ref()
                    .map(|locator| locator.as_str().to_owned()),
            });
            let status =
                (current.status == AgentStatus::Working).then_some(AgentStatus::WaitingApproval);
            if let Some(to) = status {
                logs.push(AgentLogKind::StatusChanged {
                    from: current.status,
                    to,
                });
            }
            changed(
                agent,
                logs,
                AgentMessageAssembly::default(),
                status,
                Some(turn),
                false,
                appended_at,
            )
        }
        ConversationUpdate::TurnEnded(completed)
        | ConversationUpdate::TurnEndedForPrompt {
            response: completed,
            ..
        } => {
            let turn = active_turn(current)?;
            let mut agent = current.clone();
            let mut logs = flush_message(assembly);
            let interrupted = completed.stop_reason == layer3_adapter::TurnStopReason::Cancelled;
            observe_operation(
                &mut agent,
                prompt_evidence,
                interrupted,
                appended_at,
                &mut logs,
            )?;
            if let Some(usage) = completed
                .usage
                .as_ref()
                .filter(|_| agent.conversation.observations.usage.is_none())
            {
                logs.push(AgentLogKind::Usage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    context_used: 0,
                    context_window: None,
                });
            }
            if interrupted {
                fail_first_message(
                    &mut agent,
                    "agent_turn_failed",
                    "the first-message turn was interrupted",
                    appended_at,
                    &mut logs,
                )?;
                logs.push(AgentLogKind::TurnInterrupted {
                    turn_sequence: turn,
                });
            } else {
                complete_first_message(&mut agent, appended_at, &mut logs)?;
                logs.push(AgentLogKind::TurnCompleted {
                    turn_sequence: turn,
                });
            }
            logs.push(AgentLogKind::StatusChanged {
                from: current.status,
                to: AgentStatus::Ready,
            });
            changed(
                agent,
                logs,
                AgentMessageAssembly::default(),
                Some(AgentStatus::Ready),
                None,
                true,
                appended_at,
            )
        }
        ConversationUpdate::Failed {
            message,
            capability,
        }
        | ConversationUpdate::FailedForPrompt {
            message,
            capability,
            ..
        } => {
            let turn = active_turn(current)?;
            let mut agent = current.clone();
            let mut logs = flush_message(assembly);
            observe_operation(&mut agent, prompt_evidence, false, appended_at, &mut logs)?;
            let class = capability
                .clone()
                .unwrap_or_else(|| "agent_turn_failed".into());
            fail_first_message(&mut agent, &class, message, appended_at, &mut logs)?;
            logs.push(AgentLogKind::TurnFailed {
                turn_sequence: turn,
                failure: super::TypedFailure {
                    class,
                    detail: bounded(message, 4 * 1024),
                },
            });
            logs.push(AgentLogKind::StatusChanged {
                from: current.status,
                to: AgentStatus::Ready,
            });
            changed(
                agent,
                logs,
                AgentMessageAssembly::default(),
                Some(AgentStatus::Ready),
                None,
                true,
                appended_at,
            )
        }
        ConversationUpdate::Lost { frames } => changed(
            current.clone(),
            vec![AgentLogKind::ConversationGap { frames: *frames }],
            assembly.clone(),
            None,
            current.conversation.active_turn,
            false,
            appended_at,
        ),
        ConversationUpdate::SignInRequired { url, code, message } => changed(
            current.clone(),
            vec![AgentLogKind::SignInRequired {
                url: bounded(url, 4 * 1024),
                code: code.as_ref().map(|value| bounded(value, 512)),
                message: message.as_ref().map(|value| bounded(value, 4 * 1024)),
            }],
            assembly.clone(),
            None,
            current.conversation.active_turn,
            false,
            appended_at,
        ),
        ConversationUpdate::WithoutPromptEvidence(_) | ConversationUpdate::RetainedEffect(_) => {
            unreachable!("wrapper removed")
        }
        ConversationUpdate::RuntimeOutput { .. } => Err(AgentConversationError::InvalidState(
            "runtime output receipt must be recorded before folding",
        )),
    }
}

fn fold_message_chunk(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    chunk: &layer3_adapter::ContentChunk,
    prompt_evidence: bool,
    appended_at: &str,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    active_turn(current)?;
    let layer3_adapter::ContentBlock::Text(text) = &chunk.content else {
        return Ok(FoldedConversationUpdate::Quiet(assembly.clone()));
    };
    let message_id = chunk.message_id.as_ref().map(|id| id.0.to_string());
    let mut agent = current.clone();
    let mut next_assembly = assembly.clone();
    let mut logs = Vec::new();
    if !next_assembly.content.is_empty() && next_assembly.message_id != message_id {
        logs.extend(flush_message(&next_assembly));
        next_assembly = AgentMessageAssembly::default();
    }
    next_assembly.message_id = message_id;
    next_assembly.prompt_evidence =
        (next_assembly.content.is_empty() || next_assembly.prompt_evidence) && prompt_evidence;
    append_bounded(
        &mut next_assembly.content,
        &text.text,
        MAX_DURABLE_AGENT_MESSAGE_BYTES,
    );
    observe_first_message(&mut agent, prompt_evidence, appended_at, &mut logs)?;
    observe_operation(&mut agent, prompt_evidence, false, appended_at, &mut logs)?;
    if logs.is_empty() {
        return Ok(FoldedConversationUpdate::Quiet(next_assembly));
    }
    changed(
        agent,
        logs,
        next_assembly,
        None,
        current.conversation.active_turn,
        false,
        appended_at,
    )
}

fn fold_semantic_update(
    current: &Agent,
    assembly: &AgentMessageAssembly,
    update: &SessionUpdate,
    prompt_evidence: bool,
    appended_at: &str,
    durable_tool_output: bool,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    let mut agent = current.clone();
    let mut logs = flush_message(assembly);
    observe_first_message(&mut agent, prompt_evidence, appended_at, &mut logs)?;
    observe_operation(&mut agent, prompt_evidence, false, appended_at, &mut logs)?;
    let initial_output = match update {
        SessionUpdate::ToolCall(call)
            if durable_tool_output && layer3_adapter::has_reported_initial_output(call) =>
        {
            recorded_initial_tool_output(call).map(|output| AgentLogKind::ToolOutputReported {
                call_id: call.tool_call_id.0.to_string(),
                output,
            })
        }
        _ => None,
    };
    let kind = match update {
        SessionUpdate::ToolCall(call) => AgentLogKind::ToolCallStarted {
            call_id: call.tool_call_id.0.to_string(),
            title: bounded(&call.title, 4 * 1024),
            kind: wire_name(&call.kind),
            status: wire_name(&call.status),
        },
        SessionUpdate::ToolCallUpdate(update)
            if update.fields.status.is_some() || update.fields.title.is_some() =>
        {
            AgentLogKind::ToolCallUpdated {
                call_id: update.tool_call_id.0.to_string(),
                status: update
                    .fields
                    .status
                    .as_ref()
                    .map(wire_name)
                    .unwrap_or_else(|| "updated".into()),
                output: durable_tool_output
                    .then(|| recorded_tool_output(&update.fields))
                    .flatten(),
                title: update
                    .fields
                    .title
                    .as_ref()
                    .map(|value| bounded(value, 4 * 1024)),
            }
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let Some(output) = durable_tool_output
                .then(|| recorded_tool_output(&update.fields))
                .flatten()
            else {
                return if logs.is_empty() {
                    Ok(FoldedConversationUpdate::Quiet(assembly.clone()))
                } else {
                    changed(
                        agent,
                        logs,
                        AgentMessageAssembly::default(),
                        None,
                        current.conversation.active_turn,
                        false,
                        appended_at,
                    )
                };
            };
            AgentLogKind::ToolOutputReported {
                call_id: update.tool_call_id.0.to_string(),
                output,
            }
        }
        SessionUpdate::UsageUpdate(usage) => AgentLogKind::Usage {
            input_tokens: 0,
            output_tokens: 0,
            context_used: usage.used,
            context_window: (usage.size > 0).then_some(usage.size),
        },
        SessionUpdate::CurrentModeUpdate(update) => AgentLogKind::ModeChanged {
            mode: update.current_mode_id.0.to_string(),
        },
        SessionUpdate::Plan(plan) => AgentLogKind::PlanSet {
            entries: plan
                .entries
                .iter()
                .map(|entry| AgentPlanStep {
                    title: bounded(&entry.content, 4 * 1024),
                    status: wire_name(&entry.status),
                })
                .collect(),
        },
        SessionUpdate::ConfigOptionUpdate(update) => AgentLogKind::ConfigOptionsSet {
            options: update
                .config_options
                .iter()
                .map(|option| AgentConfigOption {
                    id: option.id.0.to_string(),
                    name: bounded(&option.name, 512),
                    kind: wire_name(&option.kind),
                })
                .collect(),
        },
        SessionUpdate::AvailableCommandsUpdate(update) => AgentLogKind::AvailableCommandsSet {
            commands: update
                .available_commands
                .iter()
                .map(|command| AgentAvailableCommand {
                    name: bounded(&command.name, 512),
                    description: bounded(&command.description, 4 * 1024),
                })
                .collect(),
        },
        _ => {
            return if logs.is_empty() {
                Ok(FoldedConversationUpdate::Quiet(assembly.clone()))
            } else {
                changed(
                    agent,
                    logs,
                    AgentMessageAssembly::default(),
                    None,
                    current.conversation.active_turn,
                    false,
                    appended_at,
                )
            };
        }
    };
    logs.push(kind);
    logs.extend(initial_output);
    changed(
        agent,
        logs,
        AgentMessageAssembly::default(),
        None,
        current.conversation.active_turn,
        false,
        appended_at,
    )
}

pub(super) fn changed(
    mut agent: Agent,
    mut logs: Vec<AgentLogKind>,
    assembly: AgentMessageAssembly,
    status: Option<AgentStatus>,
    active_turn: Option<u64>,
    turn_closed: bool,
    appended_at: &str,
) -> Result<FoldedConversationUpdate, AgentConversationError> {
    if turn_closed {
        super::conversation_fold_support::settle_turn_policy(&mut agent, &mut logs)?;
    }
    if let Some(task) = agent.conversation.observations.task_acceptance.as_mut() {
        task.observe_execution(agent.conversation.active_turn, &logs);
    }
    agent.conversation.active_turn = active_turn;
    if let Some(status) = status {
        agent
            .transition(status, appended_at.to_owned(), None)
            .map_err(|_| AgentConversationError::InvalidState("status transition was refused"))?;
    } else {
        agent.revision = agent
            .revision
            .checked_add(1)
            .ok_or(AgentConversationError::Corrupt(
                "agent revision overflow".into(),
            ))?;
        agent.updated_at = appended_at.to_owned();
    }
    agent.conversation.last_seq = agent
        .conversation
        .last_seq
        .checked_add(
            u64::try_from(logs.len())
                .map_err(|_| AgentConversationError::Corrupt("agent log overflow".into()))?,
        )
        .ok_or(AgentConversationError::Corrupt("agent log overflow".into()))?;
    Ok(FoldedConversationUpdate::Changed(Box::new(
        ConversationChange {
            agent,
            logs,
            assembly,
            turn_closed,
        },
    )))
}
