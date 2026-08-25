//! Renewing the credential a running session presents on its owner's behalf.
//!
//! Separated from the session service so that file stays about sessions, and
//! because this is the one place the core knowingly mutates something it never
//! interprets. The value is opaque here: the core forwards it, and only the
//! runtime and the provider ever understand it.

use crate::ids::{KernelId, SessionId, SubjectId};
use crate::service::SessionService;

impl SessionService {
    /// Hand a running runtime the credential it should present from now on.
    ///
    /// Asked of the adapter rather than pushed down the terminal channel. The
    /// credential's consumer was always the supervisor's proxy; it travelled as
    /// terminal input only because the terminal was the sole channel there was,
    /// which made it impossible to give one to a runtime that renders no
    /// terminal at all.
    ///
    /// Reports whether it arrived. A credential that quietly failed to land
    /// leaves a session that looks alive and refuses every call the agent makes
    /// on its owner's behalf.
    async fn deliver_credential(&self, id: &SessionId, credential: &str) -> bool {
        let placed = {
            let entries = self.entries.read().await;
            entries
                .get(id)
                .and_then(|e| e.runtime.clone().map(|h| (e.session.kernel.clone(), h)))
        };
        let Some((kernel, handle)) = placed else {
            tracing::warn!(session_id = %id, "no runtime to give a credential to");
            return false;
        };
        let Some(adapter) = self.adapter_for(&kernel) else {
            tracing::warn!(session_id = %id, %kernel, "no adapter to give a credential through");
            return false;
        };
        match adapter.refresh_credential(&handle, credential).await {
            Ok(()) => true,
            Err(e) => {
                // Never logged with its value — only that one did not arrive.
                tracing::warn!(session_id = %id, error = %e, "could not deliver the credential");
                false
            }
        }
    }

    /// The adapter backing a kernel, if this deployment has one.
    fn adapter_for(
        &self,
        kernel: &KernelId,
    ) -> Option<std::sync::Arc<dyn layer3_adapter::KernelAdapter>> {
        self.adapters.get(kernel).cloned()
    }
    /// The credential to present on an owner's behalf, if one can be had.
    pub(crate) async fn credential_for(
        &self,
        session: &SessionId,
        owner: &SubjectId,
        services: &[crate::environment::ServiceId],
        caller_credential: Option<&str>,
    ) -> Result<Option<String>, crate::store::StoreError> {
        let Some(source) = self.credentials.as_ref() else {
            return Ok(None);
        };
        source
            .credential_for(session, owner, services, caller_credential)
            .await
    }

    /// What this session's profile says its credential may reach.
    ///
    /// Read from the session rather than from the request that created it, so
    /// a credential minted long afterwards — for a replaced runtime, or after
    /// a restart — is scoped the same way the first one was.
    /// Whether this session still exists and could present a credential.
    ///
    /// A terminated session is not live even while its record remains: the
    /// record is history, and history presents nothing.
    pub async fn session_is_live(&self, id: &SessionId) -> bool {
        match self.store.get(id).await {
            Ok(Some(record)) => record.session.status.wants_runtime(),
            _ => false,
        }
    }

    /// Whether a session still has a claim on a credential minted for it.
    ///
    /// Deliberately not `session_is_live`. That asks whether a runtime should
    /// be running, which a paused session answers no to — and a paused session
    /// is one its owner deliberately kept. Reclaiming on that answer took back
    /// the key of every stopped session as soon as the process holding the
    /// in-memory record of it restarted, so the owner came back to a session
    /// whose credential had been revoked underneath them.
    ///
    /// A credential is unclaimed only when the session it was minted for has
    /// ended or is no longer there at all.
    pub async fn session_claims_credential(
        &self,
        id: &SessionId,
    ) -> Result<bool, crate::store::StoreError> {
        Ok(self
            .store
            .get(id)
            .await?
            .is_some_and(|record| !record.session.status.is_terminal()))
    }

    pub async fn session_services(&self, id: &SessionId) -> Vec<crate::environment::ServiceId> {
        self.services_for(id).await
    }

    /// Hand one session a credential that was minted for it.
    ///
    /// Separate from the subject-wide push because a per-session credential is
    /// not shared: replacing every session's value when one was reissued would
    /// give each of them somebody else's.
    pub async fn push_session_credential(&self, id: &SessionId, credential: String) -> bool {
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return false;
            };
            entry.subject_credential = Some(credential.clone());
        }
        self.deliver_credential(id, &credential).await
    }

    pub(crate) async fn services_for(&self, id: &SessionId) -> Vec<crate::environment::ServiceId> {
        let Ok(Some(record)) = self.store.get(id).await else {
            return Vec::new();
        };
        // The session's own answer, not its profile's. A credential minted long
        // afterwards — for a replaced runtime, or after a restart — is scoped
        // the way the first one was, even if the bundle it was started from has
        // been widened or narrowed since.
        if let Some(granted) = record.session.granted_services.clone() {
            return granted;
        }
        // Recorded before the grant was written down. Falling back to the
        // profile is the only answer available, and it is better than none: a
        // session already running must not lose its model endpoint because
        // this field did not exist when it started.
        self.environments
            .services_for(
                &record.session.owner,
                record.session.environment_profile.as_ref(),
            )
            .await
    }

    /// Hand a session the credential its owner currently has.
    ///
    /// A session recovered after a restart has no credential: the value it was
    /// given belonged to a process that is gone. Until one arrives, every call
    /// the agent makes on its owner's behalf is refused, so the session looks
    /// broken while being perfectly alive.
    pub(crate) async fn supply_credential(&self, id: &SessionId, owner: &SubjectId) {
        let services = self.services_for(id).await;
        let credential = match self.credential_for(id, owner, &services, None).await {
            Ok(Some(credential)) => credential,
            Ok(None) => {
                tracing::debug!(session_id = %id, "no credential held for this session's owner");
                return;
            }
            Err(error) => {
                tracing::error!(
                    session_id = %id,
                    error = %error,
                    "could not read or mint this session's credential"
                );
                return;
            }
        };
        {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return;
            };
            entry.subject_credential = Some(credential.clone());
        }
        if self.deliver_credential(id, &credential).await {
            tracing::info!(session_id = %id, "credential supplied to a recovered session");
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
        let targets: Vec<SessionId> = {
            let mut entries = self.entries.write().await;
            entries
                .values_mut()
                .filter(|e| e.session.is_owned_by(subject) && !e.session.status.is_terminal())
                .map(|e| {
                    e.subject_credential = Some(credential.clone());
                    e.session.id.clone()
                })
                .collect()
        };

        let mut delivered = 0;
        for id in targets {
            if self.deliver_credential(&id, &credential).await {
                delivered += 1;
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
