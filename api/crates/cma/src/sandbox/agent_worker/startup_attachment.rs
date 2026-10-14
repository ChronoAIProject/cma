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
        let request = validated_open_request(agent, context)?;
        let recovered_prompt = agent.conversation.active_turn.map(|turn| {
            layer3_adapter::PromptIdentity::new(
                &target.conversation_id,
                target.runtime_generation,
                turn,
            )
        });
        let (channel, updates) = self
            .reconnect(
                context,
                connection_fence,
                &request,
                (&target.conversation_id, recovered_prompt),
            )
            .await?;
        if let Some(settings) = &agent.conversation.settings {
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
            updates,
        )
    }

    async fn reconnect(
        &self,
        context: &AgentContext,
        authority_fence: u64,
        request: &AgentOpenRequest,
        (conversation_id, recovered_prompt): (&str, Option<layer3_adapter::PromptIdentity>),
    ) -> Result<
        (
            ConversationChannel,
            mpsc::Receiver<layer3_adapter::ConversationUpdate>,
        ),
        WorkerFailure,
    > {
        let (updates, receiver) = mpsc::channel(UPDATE_BUFFER);
        match tokio::time::timeout(
            self.config.open_timeout,
            self.dependencies.adapter.reconnect_agent_with_prompt(
                &context.runtime,
                authority_fence,
                request,
                conversation_id,
                recovered_prompt,
                updates,
            ),
        )
        .await
        {
            Ok(Ok(channel)) => Ok((channel, receiver)),
            Ok(Err(error)) => Err(adapter_failure(error)),
            Err(_) => Err(WorkerFailure::retryable(
                "agent_conversation_open_timeout",
                "conversation did not become ready within 120 seconds",
            )),
        }
    }

    pub(super) fn attach(
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
