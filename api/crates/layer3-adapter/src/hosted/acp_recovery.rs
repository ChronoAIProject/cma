use super::{Bridge, Outcome};
use crate::hosted::update_relay::UpdateRelay;
use crate::PromptIdentity;

/// Only the exact durable active prompt can recover across an API attachment.
/// Its result may be in the runtime backlog before initialize answers; release
/// it only once the complete handshake verifies the retained conversation.
pub(super) struct PromptRecovery {
    expected: Option<PromptIdentity>,
    /// Prompt identity admitted by this attachment. Keep it after the local
    /// waiter disappears so a late response remains turn-scoped.
    active: Option<PromptIdentity>,
    /// Terminal identity already emitted by this attachment. Duplicate
    /// responses are harmless replays and must not create another terminal.
    completed: Option<PromptIdentity>,
    verified: bool,
    /// A terminal observed before handshake verification, paired with the
    /// exact native identity carried by that response. `completed` tracks the
    /// latest observed terminal for duplicate suppression, so it must never be
    /// consulted to label this pending result.
    outcome: Option<(PromptIdentity, Outcome)>,
}

impl PromptRecovery {
    pub(super) fn new(expected: Option<PromptIdentity>) -> Self {
        Self {
            active: expected.clone(),
            expected,
            completed: None,
            verified: false,
            outcome: None,
        }
    }

    pub(super) fn remember(&mut self, prompt_id: PromptIdentity) {
        self.active = Some(prompt_id);
    }

    fn active_matches(&self, id: &str) -> bool {
        self.active
            .as_ref()
            .is_some_and(|prompt_id| prompt_id.as_str() == id)
    }

    pub(super) fn clear_active_if(&mut self, id: &str) {
        if self.active_matches(id) {
            self.active = None;
        }
    }

    pub(super) fn mark_completed(&mut self, id: &str) {
        if let Some(prompt_id) = PromptIdentity::parse(id) {
            self.active = None;
            self.completed = Some(prompt_id);
        }
    }

    pub(super) fn was_completed(&self, id: &str) -> bool {
        self.completed
            .as_ref()
            .is_some_and(|prompt_id| prompt_id.as_str() == id)
    }
}

impl Bridge {
    pub(in super::super) async fn remember_prompt(&self, prompt_id: PromptIdentity) {
        self.recovery.lock().await.remember(prompt_id);
    }

    pub(in super::super) async fn clear_active_prompt_if(&self, id: &str) {
        self.recovery.lock().await.clear_active_if(id);
    }

    pub(in super::super) async fn mark_prompt_completed(&self, id: &str) {
        if let Some(state) = &self.permission_state {
            state.lock().await.complete_prompt(id);
        }
        self.recovery.lock().await.mark_completed(id);
    }

    pub(in super::super) async fn prompt_was_completed(&self, id: &str) -> bool {
        self.recovery.lock().await.was_completed(id)
    }

    pub(in super::super) async fn recover_prompt_result(
        &self,
        id: &str,
        frame: serde_json::Value,
        updates: &UpdateRelay,
    ) -> bool {
        let mut recovery = self.recovery.lock().await;
        if recovery.expected.as_ref().map(PromptIdentity::as_str) != Some(id) {
            return false;
        }
        let prompt_id = recovery.expected.clone().expect("identity was checked");
        if let Some(state) = &self.permission_state {
            state.lock().await.complete_prompt(id);
        }
        recovery.mark_completed(id);
        recovery.expected = None;
        let outcome = crate::hosted::acp_turn::turn_outcome(frame);
        if recovery.verified {
            updates.send(crate::hosted::acp_turn::scoped_turn_outcome(
                outcome,
                true,
                Some(prompt_id),
            ));
        } else {
            recovery.outcome = Some((prompt_id, outcome));
        }
        true
    }

    pub(super) async fn release_recovered_prompt(&self, updates: &UpdateRelay) {
        let mut recovery = self.recovery.lock().await;
        recovery.verified = true;
        if let Some((prompt_id, outcome)) = recovery.outcome.take() {
            updates.send(crate::hosted::acp_turn::scoped_turn_outcome(
                outcome,
                true,
                Some(prompt_id),
            ));
        }
    }
}
