//! Durable custody for the credential one session presents.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::store::StoreError;
use layer2_core::{ServiceId, SessionId, SubjectId};
use sha2::{Digest, Sha256};

use super::persistence::{
    AuthStore, CredentialLease, DurableCredential, RevokeDebt, SecretValue, SessionCredentialState,
};
use super::{random_token, AuthState};

/// Longer than the issuer's HTTP deadline, so an ordinary slow response never
/// allows a second worker to overlap the first provider mint.
const MINT_LEASE_LIFETIME: Duration = Duration::from_secs(45);
const LEASE_POLL_INTERVAL: Duration = Duration::from_millis(25);

fn empty_state(session: &SessionId) -> SessionCredentialState {
    SessionCredentialState {
        session: session.clone(),
        generation: 0,
        current: None,
        pending: None,
        revoke_debt: Vec::new(),
        lease: None,
        retired: false,
    }
}

fn next_generation(generation: u64) -> Result<u64, StoreError> {
    generation
        .checked_add(1)
        .ok_or_else(|| StoreError::Malformed("credential generation exhausted".into()))
}

fn scope_fingerprint(services: &[ServiceId]) -> String {
    let mut names: Vec<&str> = services.iter().map(|service| service.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let mut digest = Sha256::new();
    digest.update(b"cma-session-credential-scope\0");
    for name in names {
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(name.as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn lease_matches(state: &SessionCredentialState, holder: &str, fence: u64) -> bool {
    state
        .lease
        .as_ref()
        .is_some_and(|lease| lease.holder == holder && lease.fence == fence)
}

fn queue_debt(state: &mut SessionCredentialState, credential: &DurableCredential) {
    if state
        .revoke_debt
        .iter()
        .any(|debt| debt.id == credential.id)
    {
        return;
    }
    state.revoke_debt.push(RevokeDebt {
        id: credential.id.clone(),
        owner: credential.owner.clone(),
        queued_at: SystemTime::now(),
    });
}

async fn ensure_state(
    store: &Arc<dyn AuthStore>,
    session: &SessionId,
) -> Result<SessionCredentialState, StoreError> {
    loop {
        if let Some(state) = store.credential(session).await? {
            return Ok(state);
        }
        let state = empty_state(session);
        if store.insert_credential(state.clone()).await? {
            return Ok(state);
        }
    }
}

async fn replace_state(
    store: &Arc<dyn AuthStore>,
    current: &SessionCredentialState,
    mut next: SessionCredentialState,
) -> Result<bool, StoreError> {
    next.generation = next_generation(current.generation)?;
    store.replace_credential(current.generation, next).await
}

async fn clear_owned_lease(
    store: &Arc<dyn AuthStore>,
    session: &SessionId,
    holder: &str,
    fence: u64,
) -> Result<(), StoreError> {
    loop {
        let Some(current) = store.credential(session).await? else {
            return Ok(());
        };
        if !lease_matches(&current, holder, fence) {
            return Ok(());
        }
        let mut next = current.clone();
        next.lease = None;
        if replace_state(store, &current, next).await? {
            return Ok(());
        }
    }
}

async fn queue_uncommitted(
    store: &Arc<dyn AuthStore>,
    session: &SessionId,
    credential: &DurableCredential,
) -> Result<(), StoreError> {
    loop {
        let mut current = ensure_state(store, session).await?;
        if current
            .current
            .as_ref()
            .is_some_and(|held| held.id == credential.id)
            || current
                .pending
                .as_ref()
                .is_some_and(|held| held.id == credential.id)
            || current
                .revoke_debt
                .iter()
                .any(|debt| debt.id == credential.id)
        {
            return Ok(());
        }
        let before = current.clone();
        queue_debt(&mut current, credential);
        if replace_state(store, &before, current).await? {
            return Ok(());
        }
    }
}

async fn commit_mint(
    store: &Arc<dyn AuthStore>,
    session: &SessionId,
    holder: &str,
    fence: u64,
    credential: DurableCredential,
    replacement: bool,
) -> Result<Option<String>, StoreError> {
    loop {
        let mut current = ensure_state(store, session).await?;
        if current.retired || !lease_matches(&current, holder, fence) {
            queue_uncommitted(store, session, &credential).await?;
            return Ok(None);
        }

        let value = credential.value.expose().to_string();
        let before = current.clone();
        current.lease = None;
        if replacement {
            if let Some(displaced) = current.pending.take() {
                queue_debt(&mut current, &displaced);
            }
            current.pending = Some(credential.clone());
        } else if current.current.is_none() {
            current.current = Some(credential.clone());
        } else {
            queue_debt(&mut current, &credential);
            if replace_state(store, &before, current).await? {
                return Ok(None);
            }
            continue;
        }

        if replace_state(store, &before, current).await? {
            return Ok(Some(value));
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_mint(
    store: Arc<dyn AuthStore>,
    issuer: Arc<super::keys::KeyIssuer>,
    session: SessionId,
    subject: SubjectId,
    owner_token: String,
    services: Vec<ServiceId>,
    fingerprint: String,
    holder: String,
    fence: u64,
    replacement: bool,
) -> Result<Option<String>, StoreError> {
    let Some(minted) = issuer
        .mint(&session, &subject, &owner_token, &services)
        .await
    else {
        clear_owned_lease(&store, &session, &holder, fence).await?;
        return Ok(None);
    };
    let credential = DurableCredential {
        id: minted.id,
        value: SecretValue::new(minted.full_key),
        owner: subject.clone(),
        issued_at: SystemTime::now(),
        expires_at: None,
        scope_fingerprint: fingerprint,
    };

    match commit_mint(
        &store,
        &session,
        &holder,
        fence,
        credential.clone(),
        replacement,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => {
            // A store failure after provider success is the response-loss case
            // tracked by #528. Best-effort revoke narrows it; neither the key
            // value nor provider identifier is emitted at this boundary.
            if !issuer.revoke(&credential.id, &owner_token).await {
                tracing::error!(
                    session_id = %session,
                    %subject,
                    "a minted credential could neither be persisted nor revoked"
                );
            }
            Err(error)
        }
    }
}

async fn retire_state(store: Arc<dyn AuthStore>, session: SessionId) -> Result<(), StoreError> {
    loop {
        let mut current = ensure_state(&store, &session).await?;
        if current.retired
            && current.current.is_none()
            && current.pending.is_none()
            && current.lease.is_none()
        {
            return Ok(());
        }
        let before = current.clone();
        if let Some(held) = current.current.take() {
            queue_debt(&mut current, &held);
        }
        if let Some(held) = current.pending.take() {
            queue_debt(&mut current, &held);
        }
        current.lease = None;
        current.retired = true;
        if replace_state(&store, &before, current).await? {
            return Ok(());
        }
    }
}

impl AuthState {
    pub(super) async fn mint_session_credential(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        replacement: bool,
    ) -> Result<Option<String>, StoreError> {
        let Some(issuer) = self.issuer.read().await.clone() else {
            return self.access_token(subject).await;
        };
        let fingerprint = scope_fingerprint(services);
        let mut owner_token = None;

        let (holder, fence) = loop {
            let current = ensure_state(&self.store, session).await?;
            if current.retired {
                return Ok(None);
            }
            if replacement {
                if let Some(pending) = current.pending {
                    return Ok(Some(pending.value.into_inner()));
                }
                let Some(held) = current.current.as_ref() else {
                    return Ok(None);
                };
                if held.scope_fingerprint != fingerprint {
                    return Err(StoreError::Malformed(
                        "recorded credential does not match the session grant".into(),
                    ));
                }
            } else if let Some(held) = current.current {
                if held.scope_fingerprint != fingerprint {
                    return Err(StoreError::Malformed(
                        "recorded credential does not match the session grant".into(),
                    ));
                }
                return Ok(Some(held.value.into_inner()));
            }

            if current
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > SystemTime::now())
            {
                tokio::time::sleep(LEASE_POLL_INTERVAL).await;
                continue;
            }

            if owner_token.is_none() {
                owner_token = self.access_token(subject).await?;
                if owner_token.is_none() {
                    return Ok(None);
                }
            }

            let holder = random_token();
            let fence = next_generation(current.generation)?;
            let now = SystemTime::now();
            let mut next = current.clone();
            next.lease = Some(CredentialLease {
                holder: holder.clone(),
                fence,
                acquired_at: now,
                expires_at: now + MINT_LEASE_LIFETIME,
                scope_fingerprint: fingerprint.clone(),
            });
            if replace_state(&self.store, &current, next).await? {
                break (holder, fence);
            }
        };

        let task = tokio::spawn(finish_mint(
            Arc::clone(&self.store),
            issuer,
            session.clone(),
            subject.clone(),
            owner_token.expect("a mint lease is acquired only with an owner token"),
            services.to_vec(),
            fingerprint,
            holder,
            fence,
            replacement,
        ));
        task.await.map_err(|error| {
            StoreError::Unavailable(format!("credential mint task failed: {error}"))
        })?
    }

    pub(super) async fn promote_pending_credential(
        &self,
        session: &SessionId,
    ) -> Result<bool, StoreError> {
        loop {
            let Some(mut current) = self.store.credential(session).await? else {
                return Ok(false);
            };
            let Some(replacement) = current.pending.take() else {
                return Ok(false);
            };
            let before = current.clone();
            if current.retired {
                queue_debt(&mut current, &replacement);
                if replace_state(&self.store, &before, current).await? {
                    return Ok(false);
                }
                continue;
            }
            if let Some(displaced) = current.current.replace(replacement) {
                queue_debt(&mut current, &displaced);
            }
            if replace_state(&self.store, &before, current).await? {
                return Ok(true);
            }
        }
    }

    pub(super) async fn revoke_credential_debt(&self) -> Result<usize, StoreError> {
        let _sweep = self.credential_sweep.lock().await;
        let Some(issuer) = self.issuer.read().await.clone() else {
            return Ok(0);
        };
        let states = self.store.credentials().await?;
        let mut taken = 0;
        let mut owner_tokens: HashMap<SubjectId, Option<String>> = HashMap::new();
        for state in states {
            for debt in state.revoke_debt {
                let token = if let Some(cached) = owner_tokens.get(&debt.owner) {
                    cached.clone()
                } else {
                    let loaded = self.access_token(&debt.owner).await?;
                    owner_tokens.insert(debt.owner.clone(), loaded.clone());
                    loaded
                };
                let Some(token) = token else {
                    continue;
                };
                if !issuer.revoke(&debt.id, &token).await {
                    continue;
                }

                loop {
                    let Some(mut current) = self.store.credential(&state.session).await? else {
                        break;
                    };
                    let before = current.clone();
                    let old_len = current.revoke_debt.len();
                    current.revoke_debt.retain(|queued| queued.id != debt.id);
                    if current.revoke_debt.len() == old_len {
                        break;
                    }
                    if replace_state(&self.store, &before, current).await? {
                        taken += 1;
                        break;
                    }
                }
            }
        }
        Ok(taken)
    }

    pub(super) async fn reclaim_unrecorded_credentials(
        &self,
        service: &Arc<layer2_core::SessionService>,
    ) -> Result<usize, StoreError> {
        let Some(issuer) = self.issuer.read().await.clone() else {
            return Ok(0);
        };
        let states = self.store.credentials().await?;

        // Read every claim before mutating custody or calling DELETE. If even
        // one session record cannot be read, this pass knows nothing is safe to
        // revoke and leaves all current values and debt untouched for retry.
        let mut unclaimed = Vec::new();
        for state in states.iter().filter(|state| !state.retired) {
            if !service.session_claims_credential(&state.session).await? {
                unclaimed.push(state.session.clone());
            }
        }
        for session in unclaimed {
            retire_state(Arc::clone(&self.store), session).await?;
        }

        // Refresh after retirement so the keys just moved to debt are treated
        // as accounted for by the provider-listing orphan pass below.
        let states = self.store.credentials().await?;
        let mut accounted_ids = HashSet::new();
        let mut tracked_sessions = HashSet::new();
        for state in &states {
            if state.current.is_some() || state.pending.is_some() || state.lease.is_some() {
                tracked_sessions.insert(state.session.clone());
            }
            if let Some(current) = &state.current {
                accounted_ids.insert(current.id.clone());
            }
            if let Some(pending) = &state.pending {
                accounted_ids.insert(pending.id.clone());
            }
            accounted_ids.extend(state.revoke_debt.iter().map(|debt| debt.id.clone()));
        }

        let owners = self.store.subjects().await?;
        let mut revocations = Vec::new();
        for owner in owners {
            let Some(tokens) = owner.tokens else {
                continue;
            };
            let token = tokens.access_token.into_inner();
            let minted = match issuer.minted_here(&token).await {
                Ok(minted) => minted,
                Err(error) => {
                    tracing::warn!(
                        subject = %owner.subject,
                        error = %error,
                        "could not inspect an owner's credentials for orphan reclaim"
                    );
                    continue;
                }
            };

            // Determine the whole owner listing before any DELETE. A session
            // store outage therefore defers every revocation from this read,
            // instead of turning an unreadable session into an unclaimed one.
            let mut candidates = Vec::new();
            for (id, session) in minted {
                if accounted_ids.contains(&id) || tracked_sessions.contains(&session) {
                    continue;
                }
                match service.session_claims_credential(&session).await {
                    Ok(true) => {}
                    Ok(false) => candidates.push(id),
                    Err(error) => return Err(error),
                }
            }
            revocations.extend(candidates.into_iter().map(|id| (id, token.clone())));
        }

        let mut reclaimed = 0;
        for (id, token) in revocations {
            if issuer.revoke(&id, &token).await {
                reclaimed += 1;
            }
        }
        Ok(reclaimed)
    }
}

#[async_trait::async_trait]
impl layer2_core::CredentialSource for AuthState {
    async fn credential_for(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential(session, subject, services, false)
            .await
    }

    async fn release(&self, session: &SessionId) -> Result<(), StoreError> {
        let task = tokio::spawn(retire_state(Arc::clone(&self.store), session.clone()));
        task.await.map_err(|error| {
            StoreError::Unavailable(format!("credential retirement task failed: {error}"))
        })?
    }
}

#[cfg(test)]
#[path = "delegation_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "reclaim_tests.rs"]
mod reclaim_tests;
