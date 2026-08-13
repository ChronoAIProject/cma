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
    /// The credential to present on an owner's behalf, if one can be had.
    pub(crate) async fn credential_for(&self, owner: &SubjectId) -> Option<String> {
        let source = self.credentials.as_ref()?;
        source.credential_for(owner).await
    }

    /// Hand a session the credential its owner currently has.
    ///
    /// A session recovered after a restart has no credential: the value it was
    /// given belonged to a process that is gone. Until one arrives, every call
    /// the agent makes on its owner's behalf is refused, so the session looks
    /// broken while being perfectly alive.
    pub(crate) async fn supply_credential(&self, id: &SessionId, owner: &SubjectId) {
        let Some(credential) = self.credential_for(owner).await else {
            tracing::debug!(session_id = %id, "no credential held for this session's owner");
            return;
        };
        let sender = {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            entry.subject_credential = Some(credential.clone());
            entry.input.clone()
        };
        match sender {
            Some(tx) => match tx.send(TerminalInput::Credential(credential)).await {
                Ok(()) => {
                    tracing::info!(session_id = %id, "credential supplied to a recovered session")
                }
                Err(_) => tracing::warn!(session_id = %id, "could not supply the credential"),
            },
            None => tracing::warn!(session_id = %id, "no input channel to supply a credential on"),
        }
    }

    /// Give a credential to every live session that has none.
    ///
    /// The safety net under [`Self::supply_credential`]: a session may be
    /// recovered before its owner has signed back in, at which point there is
    /// no credential to give it and none will arrive on its own.
    pub async fn top_up_credentials(&self) -> usize {
        if self.credentials.is_none() {
            return 0;
        }
        let missing: Vec<(SessionId, SubjectId)> = {
            let entries = self.entries.read().await;
            entries
                .values()
                .filter(|e| e.subject_credential.is_none() && !e.session.status.is_terminal())
                .map(|e| (e.session.id.clone(), e.session.owner.clone()))
                .collect()
        };
        let mut supplied = 0;
        for (id, owner) in missing {
            let before = self.entries.read().await.get(&id).is_some();
            self.supply_credential(&id, &owner).await;
            if before
                && self
                    .entries
                    .read()
                    .await
                    .get(&id)
                    .is_some_and(|e| e.subject_credential.is_some())
            {
                supplied += 1;
            }
        }
        supplied
    }

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
