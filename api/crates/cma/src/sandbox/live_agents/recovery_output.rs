//! One durable consumer owns active output from reconnect through publication.
use super::*;

pub(crate) struct RecoveryOutput {
    registry: Arc<LiveAgentRegistry>,
    attachment: LiveAgentAttachment,
    stopped: oneshot::Receiver<()>,
    armed: bool,
}

impl LiveAgentRegistry {
    pub(crate) fn begin_recovery(
        self: &Arc<Self>,
        target: &LiveAgentTarget,
        fence: u64,
        capabilities: RuntimeCapabilities,
        updates: mpsc::Receiver<ConversationUpdate>,
        recorder: Arc<dyn AgentUpdateRecorder>,
    ) -> Result<RecoveryOutput, LiveAgentRegistryError> {
        let manager_authority = self.attachment_owner(&target.agent_id)?;
        let mut state = self.lock();
        validate_authority(&state, &target.agent_id, target.runtime_generation, fence)?;
        for entries in [&state.staged, &state.entries] {
            if let Some(entry) = entries.get(&target.agent_id) {
                validate_replacement(
                    entry,
                    &target.conversation_id,
                    target.runtime_generation,
                    fence,
                )?;
                if exact_authority(entry, target.runtime_generation, fence) {
                    return Err(LiveAgentRegistryError::StaleGeneration);
                }
            }
        }
        accept_authority(
            &mut state,
            &target.agent_id,
            target.runtime_generation,
            fence,
        );
        let (activate, activated) = oneshot::channel();
        let (task, stopped) =
            updates::spawn_observed(Arc::downgrade(self), activated, updates, recorder);
        let entry = RegistryEntry {
            manager_authority,
            conversation_id: target.conversation_id.clone(),
            runtime_generation: target.runtime_generation,
            authority_fence: fence,
            capabilities,
            input: AttachmentInput::Opening,
            published: false,
            nonce: self.next_nonce.fetch_add(1, Ordering::Relaxed) + 1,
            task,
        };
        let attachment = attachment(&target.agent_id, &entry, true);
        // Reconnect takes over the original output stream before restoring its
        // checkpoint. The predecessor must not keep a second assembly alive.
        if let Some(previous) = state.entries.remove(&target.agent_id) {
            previous.task.abort();
        }
        if let Some(previous) = state.staged.insert(target.agent_id.clone(), entry) {
            previous.task.abort();
        }
        let _ = activate.send(attachment.identity());
        Ok(RecoveryOutput {
            registry: self.clone(),
            attachment,
            stopped,
            armed: true,
        })
    }
}

impl RecoveryOutput {
    pub(crate) async fn stopped(&mut self) {
        let _ = (&mut self.stopped).await;
    }

    pub(crate) fn finish(
        mut self,
        input: ConversationInputSender,
    ) -> Result<LiveAgentAttachment, LiveAgentRegistryError> {
        {
            let mut state = self.registry.lock();
            self.registry.attachment_owner(&self.attachment.agent_id)?;
            validate_authority(
                &state,
                &self.attachment.agent_id,
                self.attachment.runtime_generation,
                self.attachment.authority_fence,
            )?;
            let entry = exact_entry(&state.staged, &self.attachment)
                .filter(|entry| !entry.task.is_finished())
                .ok_or(LiveAgentRegistryError::NotAttached)?;
            if !matches!(entry.input, AttachmentInput::Opening) {
                return Err(LiveAgentRegistryError::NotAttached);
            }
            state
                .staged
                .get_mut(&self.attachment.agent_id)
                .unwrap()
                .input = AttachmentInput::Connected {
                input,
                activation: None,
            };
        }
        self.armed = false;
        Ok(self.attachment.clone())
    }
}

impl Drop for RecoveryOutput {
    fn drop(&mut self) {
        if self.armed {
            self.registry.rollback(&self.attachment);
        }
    }
}

#[cfg(test)]
#[path = "recovery_output_tests.rs"]
mod tests;
