use super::*;

impl LiveAgentRegistry {
    /// EOF can finish while the durable step is still PublishConnection. Recheck
    /// after its commit, when recovery can no longer be mistaken for pending work.
    pub(crate) async fn recover_lost_publication(&self, attachment: &LiveAgentAttachment) {
        let handler = {
            let state = self.lock();
            let lost = !state.entries.contains_key(&attachment.agent_id)
                && !state.staged.contains_key(&attachment.agent_id)
                && state
                    .watermarks
                    .get(&attachment.agent_id)
                    .is_some_and(|watermark| {
                        watermark.fenced
                            && watermark.runtime_generation == attachment.runtime_generation
                            && watermark.authority_fence == attachment.authority_fence
                    });
            lost.then(|| self.loss_handler.get().cloned()).flatten()
        };
        if let Some(handler) = handler {
            handler.publication_lost(attachment.identity()).await;
        }
    }

    pub(crate) fn publish_and_activate(
        &self,
        attachment: &LiveAgentAttachment,
    ) -> Result<(), LiveAgentRegistryError> {
        let mut state = self.lock();
        if exact_entry(&state.entries, attachment).is_some() {
            return Ok(());
        }
        if exact_entry(&state.staged, attachment).is_none() {
            return Err(LiveAgentRegistryError::NotAttached);
        }
        let mut entry = state
            .staged
            .remove(&attachment.agent_id)
            .ok_or(LiveAgentRegistryError::NotAttached)?;
        let AttachmentInput::Connected { activation, .. } = &mut entry.input else {
            entry.task.abort();
            return Err(LiveAgentRegistryError::NotAttached);
        };
        if entry.task.is_finished() {
            return Err(LiveAgentRegistryError::NotAttached);
        }
        if let Some(activation) = activation.take() {
            if activation.send(attachment.identity()).is_err() {
                entry.task.abort();
                return Err(LiveAgentRegistryError::NotAttached);
            }
        }
        entry.published = true;
        if let Some(replaced) = state.entries.insert(attachment.agent_id.clone(), entry) {
            replaced.task.abort();
        }
        Ok(())
    }

    /// Reuse only an unpublished connection that this process still owns.
    /// Published streams and recovery after a restart use the reconnect protocol.
    pub(crate) fn staged_attachment(
        &self,
        target: &LiveAgentTarget,
        authority_fence: u64,
    ) -> Result<Option<LiveAgentAttachment>, LiveAgentRegistryError> {
        if authority_fence == 0 {
            return Err(LiveAgentRegistryError::StaleGeneration);
        }
        let mut state = self.lock();
        validate_authority(
            &state,
            &target.agent_id,
            target.runtime_generation,
            authority_fence,
        )?;
        let Some(entry) = state.staged.get_mut(&target.agent_id) else {
            return Ok(None);
        };
        validate_replacement(
            entry,
            &target.conversation_id,
            target.runtime_generation,
            authority_fence,
        )?;
        if !matches!(
            entry.input,
            AttachmentInput::Connected {
                activation: Some(_),
                ..
            }
        ) || entry.runtime_generation != target.runtime_generation
            || entry.task.is_finished()
        {
            return Ok(None);
        }
        entry.authority_fence = authority_fence;
        let attachment = attachment(&target.agent_id, entry, false);
        accept_authority(
            &mut state,
            &target.agent_id,
            target.runtime_generation,
            authority_fence,
        );
        Ok(Some(attachment))
    }
}
