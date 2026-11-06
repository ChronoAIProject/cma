use super::{Bridge, Outcome};
use crate::hosted::update_relay::UpdateRelay;
use crate::PromptIdentity;

/// Only the exact durable active prompt can recover across an API attachment.
/// Its result may be in the runtime backlog before initialize answers; release
/// it only once the complete handshake verifies the retained conversation.
pub(super) struct PromptRecovery {
    expected: Option<PromptIdentity>,
    verified: bool,
    outcome: Option<Outcome>,
}

impl PromptRecovery {
    pub(super) fn new(expected: Option<PromptIdentity>) -> Self {
        Self {
            expected,
            verified: false,
            outcome: None,
        }
    }

    pub(super) fn expects(&self, id: &str) -> bool {
        self.expected
            .as_ref()
            .is_some_and(|expected| expected.as_str() == id)
    }
}

impl Bridge {
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
        recovery.expected = None;
        let outcome = crate::hosted::acp_turn::turn_outcome(frame);
        if recovery.verified {
            updates.send(outcome.into_update());
        } else {
            recovery.outcome = Some(outcome);
        }
        true
    }

    pub(super) async fn release_recovered_prompt(&self, updates: &UpdateRelay) {
        let mut recovery = self.recovery.lock().await;
        recovery.verified = true;
        if let Some(outcome) = recovery.outcome.take() {
            updates.send(outcome.into_update());
        }
    }
}
