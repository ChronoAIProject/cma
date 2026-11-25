use super::*;

impl AgentWorker {
    pub(in super::super) async fn reattach(
        &self,
        agent: &Agent,
        context: &AgentContext,
        target: &LiveAgentTarget,
    ) -> Result<LiveAgentAttachment, WorkerFailure> {
        let connection_fence = agent
            .work
            .lease
            .as_ref()
            .filter(|lease| lease.holder == self.config.holder)
            .map(|lease| lease.fence)
            .ok_or_else(|| {
                WorkerFailure::retryable(
                    "agent_lifecycle_fence_lost",
                    "agent lifecycle lease is no longer held",
                )
            })?;
        let request = self.recorded_open_request(agent, context).await?;
        let recovered_prompt = agent.conversation.active_turn.map(|turn| {
            layer3_adapter::PromptIdentity::new(
                &target.conversation_id,
                target.runtime_generation,
                turn,
            )
        });
        let (updates, mut receiver) = mpsc::channel(UPDATE_BUFFER);
        let quiet = recovered_prompt.is_none()
            && agent.status == layer2_core::sandbox::AgentStatus::Ready
            && agent.work.step == AgentWorkStep::PublishConnection;
        let restoring = async {
            let channel = self
                .reconnect(
                    context,
                    connection_fence,
                    &request,
                    (
                        &target.conversation_id,
                        recovered_prompt.clone(),
                        agent
                            .pending_managed_context()
                            .map(|(sequence, _)| sequence),
                    ),
                    updates,
                )
                .await?;
            let mut context_replayed = false;
            if let Some((sequence, input)) = agent.pending_managed_context() {
                let result = channel.input.send(input).await;
                let refresh_target = layer2_core::sandbox::AgentRuntimeTarget {
                    agent_id: agent.id.clone(),
                    conversation_id: target.conversation_id.clone(),
                    runtime_generation: target.runtime_generation,
                };
                let authority = layer2_core::sandbox::AgentWorkAuthority {
                    holder: self.config.holder.clone(),
                    fence: connection_fence,
                };
                let confirmation = match result {
                    Ok(()) => {
                        context_replayed = true;
                        self.dependencies
                            .operations
                            .confirm_managed_context_replayed(&refresh_target, &authority, sequence)
                            .await
                    }
                    Err(layer3_adapter::AdapterError::OperationIdentityConflict(_)) => {
                        self.dependencies
                            .operations
                            .retire_managed_context_replayed(&refresh_target, &authority, sequence)
                            .await
                            .map_err(|error| {
                                WorkerFailure::retryable(
                                    "managed_context_confirmation_unavailable",
                                    error.to_string(),
                                )
                            })?;
                        // This attachment permits only the now-retired operation.
                        // Discard it; the next claim performs an ordinary handshake.
                        return Err(WorkerFailure::retryable(
                            "managed_context_reconnect_required",
                            "retired context recovery requires a new attachment",
                        ));
                    }
                    Err(error) => return Err(adapter_failure(error)),
                };
                confirmation.map_err(|error| {
                    WorkerFailure::retryable(
                        "managed_context_confirmation_unavailable",
                        error.to_string(),
                    )
                })?;
            }
            if let Some(settings) = agent
                .conversation
                .settings
                .as_ref()
                .filter(|_| !context_replayed)
            {
                channel
                    .input
                    .send(settings.runtime_input())
                    .await
                    .map_err(adapter_failure)?;
                self.dependencies
                    .operations
                    .confirm_settings_replayed(
                        &layer2_core::sandbox::AgentRuntimeTarget {
                            agent_id: agent.id.clone(),
                            conversation_id: target.conversation_id.clone(),
                            runtime_generation: target.runtime_generation,
                        },
                        &layer2_core::sandbox::AgentWorkAuthority {
                            holder: self.config.holder.clone(),
                            fence: connection_fence,
                        },
                        settings,
                    )
                    .await
                    .map_err(|error| {
                        WorkerFailure::retryable(
                            "agent_settings_confirmation_unavailable",
                            error.to_string(),
                        )
                    })?;
            }
            Ok(channel)
        };
        if recovered_prompt.is_some() {
            runtime_capabilities(self.dependencies.adapter.capabilities())
                .require(agent.capabilities.iter())
                .map_err(|capability| {
                    WorkerFailure::terminal(
                        "agent_capability_grant_unavailable",
                        format!("replacement runtime does not provide {capability:?}"),
                    )
                })?;
            let mut recovery = self
                .dependencies
                .registry
                .begin_recovery(
                    target,
                    connection_fence,
                    agent.capabilities.clone(),
                    receiver,
                    self.dependencies.updates.clone(),
                )
                .map_err(registry_failure)?;
            let restoring = tokio::time::timeout(self.config.open_timeout, restoring);
            tokio::pin!(restoring);
            let channel = tokio::select! {
                biased;
                result = &mut restoring => result.map_err(|_| WorkerFailure::retryable(
                    "agent_conversation_open_timeout", "active conversation recovery timed out",
                ))??,
                _ = recovery.stopped() => return Err(WorkerFailure::retryable(
                    "agent_connection_output_unavailable", "active recovery output stopped",
                )),
            };
            let conversation_id = valid_conversation_id(channel.conversation_id)?;
            if conversation_id != target.conversation_id {
                return Err(WorkerFailure::terminal(
                    "agent_conversation_identity_mismatch",
                    "runtime replay returned another conversation",
                ));
            }
            return recovery.finish(channel.input).map_err(registry_failure);
        }
        let channel = if quiet {
            self.drain_connection_output(agent, restoring, &mut receiver)
                .await?
        } else {
            restoring.await?
        };
        let conversation_id = valid_conversation_id(channel.conversation_id)?;
        if conversation_id != target.conversation_id {
            return Err(WorkerFailure::terminal(
                "agent_conversation_identity_mismatch",
                "runtime replay returned another conversation",
            ));
        }
        self.attach(
            agent,
            conversation_id,
            target.runtime_generation,
            channel.input,
            receiver,
        )
    }

    async fn reconnect(
        &self,
        context: &AgentContext,
        authority_fence: u64,
        request: &AgentOpenRequest,
        (conversation_id, recovered_prompt, recovered_context): (
            &str,
            Option<layer3_adapter::PromptIdentity>,
            Option<u64>,
        ),
        updates: mpsc::Sender<layer3_adapter::ConversationUpdate>,
    ) -> Result<ConversationChannel, WorkerFailure> {
        match tokio::time::timeout(self.config.open_timeout, async {
            if let Some(sequence) = recovered_context {
                if recovered_prompt.is_some() {
                    return Err(layer3_adapter::AdapterError::OperationIdentityConflict(
                        "active turn cannot replay idle context".into(),
                    ));
                }
                self.dependencies
                    .adapter
                    .reconnect_agent_with_context(
                        &context.runtime,
                        authority_fence,
                        request,
                        conversation_id,
                        sequence,
                        updates,
                    )
                    .await
            } else {
                self.dependencies
                    .adapter
                    .reconnect_agent_with_prompt(
                        &context.runtime,
                        authority_fence,
                        request,
                        conversation_id,
                        recovered_prompt,
                        updates,
                    )
                    .await
            }
        })
        .await
        {
            Ok(Ok(channel)) => Ok(channel),
            Ok(Err(error)) => Err(adapter_failure(error)),
            Err(_) => Err(WorkerFailure::retryable(
                "agent_conversation_open_timeout",
                "conversation did not become ready within 120 seconds",
            )),
        }
    }

    pub(in super::super) async fn recorded_open_request(
        &self,
        agent: &Agent,
        context: &AgentContext,
    ) -> Result<AgentOpenRequest, WorkerFailure> {
        let open = agent
            .work
            .receipts
            .iter()
            .find(|receipt| receipt.step == AgentWorkStep::OpenConversation);
        let numbered = open
            .and_then(|receipt| {
                layer3_adapter::AgentProcessAction::Open
                    .incarnation(agent.id.as_str(), &receipt.operation_id)
            })
            .is_some_and(|incarnation| incarnation > 1);
        if !numbered {
            return validated_open_request(agent, context);
        }
        let invalid = || {
            WorkerFailure::retryable(
                "agent_restart_open_unavailable",
                "the exact numbered Agent Open could not be verified",
            )
        };
        let request = self
            .dependencies
            .store
            .restart_connection_open(agent)
            .await
            .map_err(|_| invalid())?
            .ok_or_else(invalid)?;
        if !request.has_valid_sha256()
            || open.is_none_or(|receipt| {
                receipt.operation_id != request.operation_id
                    || receipt.request_sha256 != request.request_sha256
            })
            || request.resume_conversation != agent.conversation.conversation_id
            || request.options != runtime_options(agent, &context.sandbox, &context.worktree)
            || request.restart_runtime.as_ref().is_none_or(|pin| {
                context.sandbox.runtime.as_ref().is_none_or(|binding| {
                    binding.boot_id.as_ref() != Some(&pin.boot_id)
                        || binding.provision_generation != Some(pin.runtime.generation)
                })
            })
        {
            return Err(invalid());
        }
        Ok(request)
    }

    pub(in crate::sandbox::agent_worker) fn attach(
        &self,
        agent: &Agent,
        conversation_id: String,
        runtime_generation: u64,
        input: layer3_adapter::ConversationInputSender,
        updates: mpsc::Receiver<layer3_adapter::ConversationUpdate>,
    ) -> Result<LiveAgentAttachment, WorkerFailure> {
        let fence = agent
            .work
            .lease
            .as_ref()
            .filter(|lease| lease.holder == self.config.holder)
            .map(|lease| lease.fence)
            .ok_or_else(|| {
                WorkerFailure::retryable(
                    "agent_lifecycle_fence_lost",
                    "agent lifecycle lease is no longer held",
                )
            })?;
        let offered = runtime_capabilities(self.dependencies.adapter.capabilities());
        offered
            .require(agent.capabilities.iter())
            .map_err(|capability| {
                WorkerFailure::terminal(
                    "agent_capability_grant_unavailable",
                    format!("replacement runtime does not provide {capability:?}"),
                )
            })?;
        self.dependencies
            .registry
            .attach(
                agent.id.clone(),
                conversation_id,
                runtime_generation,
                fence,
                agent.capabilities.clone(),
                input,
                updates,
                self.dependencies.updates.clone(),
            )
            .map_err(registry_failure)
    }
}

// A retained local channel has the same replay authority as a reconnect.
pub(in super::super) fn validated_open_request(
    agent: &Agent,
    context: &AgentContext,
) -> Result<AgentOpenRequest, WorkerFailure> {
    let request = AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        runtime_options(agent, &context.sandbox, &context.worktree),
        None,
    );
    if agent
        .work
        .receipts
        .iter()
        .find(|receipt| receipt.step == AgentWorkStep::OpenConversation)
        .filter(|receipt| {
            receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
        })
        .is_none()
    {
        return Err(WorkerFailure::terminal(
            "agent_conversation_receipt_invalid",
            "the durable conversation-open receipt does not match replay",
        ));
    }
    Ok(request)
}
