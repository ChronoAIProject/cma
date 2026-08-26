//! Renewing the credential a running session presents on its owner's behalf.
//!
//! Separated from the session service so that file stays about sessions, and
//! because this is the one place the core knowingly mutates something it never
//! interprets. The value is opaque here: the core forwards it, and only the
//! runtime and the provider ever understand it.

use std::future::Future;

use layer3_adapter::RuntimeHandle;

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
    pub(crate) async fn install_credential_while_lifecycle_locked<F, Fut>(
        &self,
        _lifecycle: &tokio::sync::OwnedMutexGuard<()>,
        id: &SessionId,
        expected_runtime: &RuntimeHandle,
        credential: &str,
        commit: F,
    ) -> Result<bool, crate::store::StoreError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<bool, crate::store::StoreError>> + Send,
    {
        let kernel = {
            let entries = self.entries.read().await;
            entries.get(id).and_then(|entry| {
                (entry.runtime.as_ref() == Some(expected_runtime))
                    .then(|| entry.session.kernel.clone())
            })
        };
        let Some(kernel) = kernel else {
            tracing::warn!(session_id = %id, "the selected runtime is no longer current for credential delivery");
            return Ok(false);
        };
        let Some(adapter) = self.adapter_for(&kernel) else {
            tracing::warn!(session_id = %id, %kernel, "no adapter to give a credential through");
            return Ok(false);
        };
        if let Err(error) = adapter
            .refresh_credential(expected_runtime, credential)
            .await
        {
            // Never logged with its value — only that one did not arrive.
            tracing::warn!(session_id = %id, %error, "could not confirm credential delivery");
            return Ok(false);
        }

        // For a replacement this is the generation-CAS promotion. It runs only
        // after H1 acknowledged the exact value and before H1 can be removed.
        // A failed promotion leaves the old entry evidence untouched.
        if !commit().await? {
            return Ok(false);
        }

        let mut entries = self.entries.write().await;
        let Some(entry) = entries
            .get_mut(id)
            .filter(|entry| entry.runtime.as_ref() == Some(expected_runtime))
        else {
            // Defensive even though lifecycle replacement obeys the same gate:
            // if a non-lifecycle path removed H1, durable promotion already
            // makes any successor select the acknowledged value.
            tracing::warn!(session_id = %id, runtime = %expected_runtime.0, "credential was acknowledged after its runtime entry changed");
            return Ok(false);
        };
        entry.subject_credential = Some(credential.to_string());
        Ok(true)
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
        caller_credential: Option<&crate::session::CallerCredential>,
    ) -> Result<Option<String>, crate::store::StoreError> {
        let Some(source) = self.credentials.as_ref() else {
            return Ok(None);
        };
        source
            .credential_for(session, owner, services, caller_credential)
            .await
    }

    /// Whether this session still exists and could present a credential.
    ///
    /// A terminated session is not live even while its record remains: the
    /// record is history, and history presents nothing.
    pub async fn session_is_live(&self, id: &SessionId) -> bool {
        match self.store.get(id).await {
            Ok(Some(record)) => record.session.wants_runtime(),
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

    /// Whether taking back an untracked sibling credential can strand this session.
    ///
    /// A session with no runtime has nothing presenting a credential, so its
    /// durable current value is enough to protect the one it will use later. A
    /// session that should be running is different: one of the values custody
    /// records must have reached its adapter before another same-session key is
    /// safe to revoke. During restart readoption, and after a failed delivery,
    /// the in-memory entry deliberately records no value and this returns false.
    pub async fn can_reclaim_untracked_credential(
        &self,
        id: &SessionId,
        tracked_values: &[&str],
    ) -> Result<bool, crate::store::StoreError> {
        let Some(record) = self.store.get(id).await? else {
            return Ok(true);
        };
        if !record.session.wants_runtime() {
            return Ok(true);
        }
        let entries = self.entries.read().await;
        let delivered = entries
            .get(id)
            .and_then(|entry| entry.subject_credential.as_deref());
        Ok(delivered.is_some_and(|delivered| tracked_values.contains(&delivered)))
    }

    pub async fn session_services(&self, id: &SessionId) -> Vec<crate::environment::ServiceId> {
        self.services_for(id).await
    }

    /// Authoritative owner and frozen grant for recurring credential work.
    ///
    /// Missing or terminal sessions have no context that can authorize a mint
    /// or delivery. Store failures remain distinguishable from that absence.
    pub async fn session_credential_context(
        &self,
        id: &SessionId,
    ) -> Result<Option<(SubjectId, Vec<crate::environment::ServiceId>)>, crate::store::StoreError>
    {
        let Some(record) = self.store.get(id).await? else {
            return Ok(None);
        };
        if record.session.status.is_terminal() {
            return Ok(None);
        }
        let owner = record.session.owner.clone();
        let services = self.services_for_record(&record).await;
        Ok(Some((owner, services)))
    }

    /// Hand one session a credential that was minted for it.
    ///
    /// Separate from the subject-wide push because a per-session credential is
    /// not shared: replacing every session's value when one was reissued would
    /// give each of them somebody else's.
    pub async fn push_session_credential(&self, id: &SessionId, credential: String) -> bool {
        self.push_session_credential_with(id, &credential, || async {
            Ok::<bool, crate::store::StoreError>(true)
        })
        .await
        .unwrap_or(false)
    }

    /// Deliver, durably commit, and record a credential on one runtime generation.
    ///
    /// `commit` is called only after the adapter acknowledges installation and
    /// while lifecycle replacement remains fenced. CMA uses it to promote the
    /// exact pending value before its predecessor can enter revoke debt.
    pub async fn push_session_credential_with<F, Fut>(
        &self,
        id: &SessionId,
        credential: &str,
        commit: F,
    ) -> Result<bool, crate::store::StoreError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<bool, crate::store::StoreError>> + Send,
    {
        let lifecycle = self.lock_lifecycle(id).await;
        let runtime = self
            .entries
            .read()
            .await
            .get(id)
            .and_then(|entry| entry.runtime.clone());
        let Some(runtime) = runtime else {
            tracing::warn!(session_id = %id, "no runtime to give a credential to");
            return Ok(false);
        };
        self.install_credential_while_lifecycle_locked(&lifecycle, id, &runtime, credential, commit)
            .await
    }

    /// Push a shared subject credential only while this subject still owns the
    /// selected live runtime. Provenance is read outside the lifecycle gate,
    /// so ownership and status must be checked again after acquiring it.
    async fn push_subject_credential(
        &self,
        id: &SessionId,
        subject: &SubjectId,
        credential: &str,
    ) -> bool {
        let lifecycle = self.lock_lifecycle(id).await;
        let runtime = self.entries.read().await.get(id).and_then(|entry| {
            (entry.session.is_owned_by(subject) && !entry.session.status.is_terminal())
                .then(|| entry.runtime.clone())
                .flatten()
        });
        let Some(runtime) = runtime else {
            return false;
        };
        self.install_credential_while_lifecycle_locked(
            &lifecycle,
            id,
            &runtime,
            credential,
            || async { Ok::<bool, crate::store::StoreError>(true) },
        )
        .await
        .unwrap_or(false)
    }

    pub(crate) async fn services_for(&self, id: &SessionId) -> Vec<crate::environment::ServiceId> {
        let Ok(Some(record)) = self.store.get(id).await else {
            return Vec::new();
        };
        self.services_for_record(&record).await
    }

    /// What this session's recorded grant says its credential may reach.
    async fn services_for_record(
        &self,
        record: &crate::store::SessionRecord,
    ) -> Vec<crate::environment::ServiceId> {
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
        if !self.push_session_credential(id, credential).await {
            tracing::warn!(
                session_id = %id,
                "credential delivery failed; keeping any predecessor eligible until retry"
            );
            return;
        }
        tracing::info!(session_id = %id, "credential supplied to a recovered session");
    }

    /// Give a credential to every live session that has none.
    ///
    /// The safety net under `Self::supply_credential`: a session may be
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

    /// Which credential this process has confirmed the runtime received.
    #[cfg(test)]
    pub(crate) async fn credential_held(&self, id: &SessionId) -> Option<String> {
        self.entries
            .read()
            .await
            .get(id)
            .and_then(|entry| entry.subject_credential.clone())
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
        let candidates: Vec<SessionId> = {
            let entries = self.entries.read().await;
            entries
                .values()
                .filter(|e| e.session.is_owned_by(subject) && !e.session.status.is_terminal())
                .map(|e| e.session.id.clone())
                .collect()
        };

        let mut targets = Vec::new();
        for id in candidates {
            let tracks_subject = match self.credentials.as_ref() {
                Some(source) => match source.tracks_subject_credential(&id).await {
                    Ok(tracks) => tracks,
                    Err(error) => {
                        tracing::warn!(
                            session_id = %id,
                            error = %error,
                            "could not determine credential provenance; renewal was not pushed"
                        );
                        false
                    }
                },
                None => false,
            };
            if tracks_subject {
                targets.push(id);
            }
        }

        let mut delivered = 0;
        for id in targets {
            if self
                .push_subject_credential(&id, subject, &credential)
                .await
            {
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
