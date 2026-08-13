//! Renewing the credential a running session presents on its owner's behalf.
//!
//! Separated from the session service so that file stays about sessions, and
//! because this is the one place the core knowingly mutates something it never
//! interprets. The value is opaque here: the core forwards it, and only the
//! runtime and the provider ever understand it.

use tokio::sync::mpsc;

use crate::ids::{SessionId, SubjectId};
use crate::service::SessionService;
use layer3_adapter::TerminalInput;

impl SessionService {
    /// Replace the delegated credential on every live session a subject owns.
    ///
    /// Called when the owner's credential is renewed. The runtime swaps it
    /// behind the agent, so a long-running session survives renewal instead of
    /// failing partway through a turn.
    pub async fn refresh_subject_credential(
        &self,
        subject: &SubjectId,
        credential: String,
    ) -> usize {
        let targets: Vec<(SessionId, mpsc::Sender<TerminalInput>)> = {
            let mut entries = self.entries.write().await;
            entries
                .values_mut()
                .filter(|e| e.session.is_owned_by(subject) && !e.session.status.is_terminal())
                .filter_map(|e| {
                    e.subject_credential = Some(credential.clone());
                    e.input.clone().map(|tx| (e.session.id.clone(), tx))
                })
                .collect()
        };

        let mut delivered = 0;
        for (id, tx) in targets {
            if tx
                .send(TerminalInput::Credential(credential.clone()))
                .await
                .is_ok()
            {
                delivered += 1;
            } else {
                tracing::warn!(session_id = %id, "could not deliver refreshed credential");
            }
        }
        if delivered > 0 {
            tracing::info!(%subject, sessions = delivered, "delegated credential refreshed");
        }
        delivered
    }
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
