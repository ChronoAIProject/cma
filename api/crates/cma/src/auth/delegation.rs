//! Durable custody for the credential one session presents.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use layer2_core::store::StoreError;
use layer2_core::{CallerCredential, ServiceId, SessionId, SubjectId};
use sha2::{Digest, Sha256};

use super::persistence::{
    AuthStore, CredentialLease, DurableCredential, RevokeDebt, RevokeDebtCursor, RevokeDebtWork,
    SecretValue, SessionCredentialState,
};
#[cfg(test)]
use super::AUTH_SWEEP_TWO_LANE_BATCH_SIZE;
use super::{
    finish_sweep_page, random_token, AuthOperationBudget, AuthState, OrphanOwnerSweep, SweepEpoch,
};

/// Longer than the issuer's HTTP deadline, so an ordinary slow response never
/// allows a second worker to overlap the first provider mint.
const MINT_LEASE_LIFETIME: Duration = Duration::from_secs(45);
const LEASE_POLL_INTERVAL: Duration = Duration::from_millis(25);

fn remember_pass_error(slot: &mut Option<StoreError>, error: StoreError, operation: &str) {
    tracing::warn!(error = %error, operation, "bounded credential reconciliation item failed");
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn take_operation(
    budget: Option<&AuthOperationBudget>,
    amount: usize,
    operation: &str,
) -> Result<(), StoreError> {
    if budget.is_some_and(|budget| !budget.take(amount)) {
        return Err(StoreError::Unavailable(format!(
            "recurring authentication operation budget exhausted before {operation}"
        )));
    }
    Ok(())
}

fn empty_state(session: &SessionId) -> SessionCredentialState {
    SessionCredentialState {
        session: session.clone(),
        generation: 0,
        request_bound_credential: None,
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

fn validate_credential_grant(
    credential: &DurableCredential,
    subject: &SubjectId,
    fingerprint: &str,
) -> Result<(), StoreError> {
    if &credential.owner != subject {
        return Err(StoreError::Malformed(
            "recorded credential does not belong to the session owner".into(),
        ));
    }
    if credential.scope_fingerprint != fingerprint {
        return Err(StoreError::Malformed(
            "recorded credential does not match the session grant".into(),
        ));
    }
    Ok(())
}

fn lease_matches(state: &SessionCredentialState, holder: &str, fence: u64) -> bool {
    state
        .lease
        .as_ref()
        .is_some_and(|lease| lease.holder == holder && lease.fence == fence)
}

fn queue_debt(state: &mut SessionCredentialState, credential: &DurableCredential) {
    let revocation_credential = credential
        .revocation_credential
        .clone()
        .or_else(|| state.request_bound_credential.clone());
    if let Some(queued) = state
        .revoke_debt
        .iter_mut()
        .find(|debt| debt.id == credential.id)
    {
        if queued.revocation_credential.is_none() {
            queued.revocation_credential = revocation_credential;
        }
        return;
    }
    let debt = RevokeDebt {
        id: credential.id.clone(),
        owner: credential.owner.clone(),
        queued_at: SystemTime::now(),
        revocation_credential,
    };
    let position = state
        .revoke_debt
        .binary_search_by(|queued| queued.id.cmp(&debt.id))
        .unwrap_or_else(|position| position);
    state.revoke_debt.insert(position, debt);
}

fn retain_request_bound_revocation_authority(state: &mut SessionCredentialState) {
    let Some(authority) = state.request_bound_credential.as_ref() else {
        return;
    };
    for debt in &mut state.revoke_debt {
        if debt.revocation_credential.is_none() {
            debt.revocation_credential = Some(authority.clone());
        }
    }
}

fn enrich_accounted_revocation_authority(
    state: &mut SessionCredentialState,
    credential: &DurableCredential,
) -> bool {
    let Some(authority) = credential
        .revocation_credential
        .clone()
        .or_else(|| state.request_bound_credential.clone())
    else {
        return false;
    };
    let mut enriched = false;
    if let Some(current) = state.current.as_mut() {
        if current.id == credential.id && current.revocation_credential.is_none() {
            current.revocation_credential = Some(authority.clone());
            enriched = true;
        }
    }
    if let Some(pending) = state.pending.as_mut() {
        if pending.id == credential.id && pending.revocation_credential.is_none() {
            pending.revocation_credential = Some(authority.clone());
            enriched = true;
        }
    }
    for debt in &mut state.revoke_debt {
        if debt.id == credential.id && debt.revocation_credential.is_none() {
            debt.revocation_credential = Some(authority.clone());
            enriched = true;
        }
    }
    enriched
}

fn state_accounts_for_provider_credential(state: &SessionCredentialState, id: &str) -> bool {
    state.current.as_ref().is_some_and(|held| held.id == id)
        || state.pending.as_ref().is_some_and(|held| held.id == id)
        || state.revoke_debt.iter().any(|debt| debt.id == id)
}

fn revoke_debt_work_position(
    item: &super::persistence::AuthSweepItem<RevokeDebtCursor, RevokeDebtWork>,
) -> Option<RevokeDebtCursor> {
    Some(item.cursor.clone())
}

async fn ensure_state(
    store: &Arc<dyn AuthStore>,
    session: &SessionId,
    budget: Option<&AuthOperationBudget>,
) -> Result<SessionCredentialState, StoreError> {
    loop {
        take_operation(budget, 1, "credential custody read")?;
        if let Some(state) = store.credential(session).await? {
            return Ok(state);
        }
        let state = empty_state(session);
        take_operation(budget, 2, "credential custody insert")?;
        if store.insert_credential(state.clone()).await? {
            return Ok(state);
        }
    }
}

async fn replace_state(
    store: &Arc<dyn AuthStore>,
    current: &SessionCredentialState,
    mut next: SessionCredentialState,
    budget: Option<&AuthOperationBudget>,
) -> Result<bool, StoreError> {
    next.generation = next_generation(current.generation)?;
    take_operation(budget, 2, "credential custody replacement")?;
    store.replace_credential(current.generation, next).await
}

async fn clear_owned_lease(
    store: &Arc<dyn AuthStore>,
    session: &SessionId,
    holder: &str,
    fence: u64,
    budget: Option<&AuthOperationBudget>,
) -> Result<(), StoreError> {
    loop {
        take_operation(budget, 1, "mint lease cleanup read")?;
        let Some(current) = store.credential(session).await? else {
            return Ok(());
        };
        if !lease_matches(&current, holder, fence) {
            return Ok(());
        }
        let mut next = current.clone();
        next.lease = None;
        if replace_state(store, &current, next, budget).await? {
            return Ok(());
        }
    }
}

async fn queue_uncommitted(
    store: &Arc<dyn AuthStore>,
    session: &SessionId,
    credential: &DurableCredential,
    mut terminal_observed: bool,
    budget: Option<&AuthOperationBudget>,
) -> Result<(), StoreError> {
    loop {
        take_operation(budget, 1, "uncommitted credential custody read")?;
        let Some(mut current) = store.credential(session).await? else {
            terminal_observed = true;
            // A settled retired tombstone can be TTL-deleted before an
            // arbitrarily delayed mint task wakes. Recreate the terminal fence
            // together with its debt; an empty active record would resurrect a
            // session that release already made terminal.
            let mut retired = empty_state(session);
            retired.retired = true;
            queue_debt(&mut retired, credential);
            take_operation(budget, 2, "uncommitted credential custody insert")?;
            if store.insert_credential(retired).await? {
                return Ok(());
            }
            continue;
        };

        terminal_observed |= current.retired;
        let credential_is_accounted_for =
            state_accounts_for_provider_credential(&current, &credential.id);
        let before = current.clone();
        let authority_enriched = enrich_accounted_revocation_authority(&mut current, credential);

        if terminal_observed {
            if current.retired
                && current.request_bound_credential.is_none()
                && current.current.is_none()
                && current.pending.is_none()
                && current.lease.is_none()
                && credential_is_accounted_for
                && !authority_enriched
            {
                return Ok(());
            }
            if let Some(held) = current.current.take() {
                queue_debt(&mut current, &held);
            }
            if let Some(held) = current.pending.take() {
                queue_debt(&mut current, &held);
            }
            retain_request_bound_revocation_authority(&mut current);
            current.request_bound_credential = None;
            current.lease = None;
            current.retired = true;
            queue_debt(&mut current, credential);
            if replace_state(store, &before, current, budget).await? {
                return Ok(());
            }
            continue;
        }

        if credential_is_accounted_for {
            if !authority_enriched {
                return Ok(());
            }
            if replace_state(store, &before, current, budget).await? {
                return Ok(());
            }
            continue;
        }
        queue_debt(&mut current, credential);
        if replace_state(store, &before, current, budget).await? {
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
    budget: Option<&AuthOperationBudget>,
) -> Result<Option<String>, StoreError> {
    loop {
        take_operation(budget, 1, "mint commit custody read")?;
        let Some(mut current) = store.credential(session).await? else {
            queue_uncommitted(store, session, &credential, true, budget).await?;
            return Ok(None);
        };
        if current.retired {
            queue_uncommitted(store, session, &credential, true, budget).await?;
            return Ok(None);
        }
        if !lease_matches(&current, holder, fence) {
            queue_uncommitted(store, session, &credential, false, budget).await?;
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
            if replace_state(store, &before, current, budget).await? {
                return Ok(None);
            }
            continue;
        }

        if replace_state(store, &before, current, budget).await? {
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
    revocation_credential: Option<String>,
    services: Vec<ServiceId>,
    fingerprint: String,
    holder: String,
    fence: u64,
    replacement: bool,
    budget: Option<AuthOperationBudget>,
) -> Result<Option<String>, StoreError> {
    take_operation(
        budget.as_ref(),
        2,
        "provider service resolution and credential mint",
    )?;
    let Some(minted) = issuer
        .mint(&session, &subject, &owner_token, &services)
        .await
    else {
        clear_owned_lease(&store, &session, &holder, fence, budget.as_ref()).await?;
        return Ok(None);
    };
    let credential = DurableCredential {
        id: minted.id,
        value: SecretValue::new(minted.full_key),
        owner: subject.clone(),
        issued_at: SystemTime::now(),
        expires_at: Some(minted.expires_at),
        scope_fingerprint: fingerprint,
        revocation_credential: revocation_credential.map(SecretValue::new),
    };

    match commit_mint(
        &store,
        &session,
        &holder,
        fence,
        credential.clone(),
        replacement,
        budget.as_ref(),
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => {
            // A store failure after provider success is the response-loss case
            // tracked by #528. The failed response is ambiguous: Mongo may
            // have applied the write and lost only its acknowledgement. Confirm
            // custody before DELETE so cleanup cannot destroy a recorded key.
            let accounted_for =
                if take_operation(budget.as_ref(), 1, "failed mint custody confirmation read")
                    .is_ok()
                {
                    match store.credential(&session).await {
                        Ok(Some(state)) => Some(state_accounts_for_provider_credential(
                            &state,
                            &credential.id,
                        )),
                        Ok(None) => Some(false),
                        Err(read_error) => {
                            tracing::warn!(
                                session_id = %session,
                                %subject,
                                error = %read_error,
                                "could not confirm whether failed mint custody was recorded"
                            );
                            None
                        }
                    }
                } else {
                    None
                };
            match accounted_for {
                Some(true) => tracing::warn!(
                    session_id = %session,
                    %subject,
                    "mint custody was recorded although its acknowledgement was lost"
                ),
                Some(false) => {
                    let revoke_attempted = take_operation(
                        budget.as_ref(),
                        1,
                        "uncommitted provider credential revocation",
                    )
                    .is_ok();
                    if !revoke_attempted || !issuer.revoke(&credential.id, &owner_token).await {
                        tracing::error!(
                            session_id = %session,
                            %subject,
                            "a minted credential could neither be persisted nor revoked"
                        );
                    }
                }
                None => tracing::error!(
                    session_id = %session,
                    %subject,
                    "mint cleanup deferred because its durable outcome is unknown"
                ),
            }
            Err(error)
        }
    }
}

async fn retire_state(store: Arc<dyn AuthStore>, session: SessionId) -> Result<(), StoreError> {
    loop {
        let mut current = ensure_state(&store, &session, None).await?;
        if current.retired
            && current.request_bound_credential.is_none()
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
        retain_request_bound_revocation_authority(&mut current);
        current.request_bound_credential = None;
        current.lease = None;
        current.retired = true;
        if replace_state(&store, &before, current, None).await? {
            return Ok(());
        }
    }
}

async fn retire_reconciled_state_once(
    store: &Arc<dyn AuthStore>,
    mut current: SessionCredentialState,
) -> Result<bool, StoreError> {
    let before = current.clone();
    if let Some(held) = current.current.take() {
        queue_debt(&mut current, &held);
    }
    if let Some(held) = current.pending.take() {
        queue_debt(&mut current, &held);
    }
    retain_request_bound_revocation_authority(&mut current);
    current.request_bound_credential = None;
    current.lease = None;
    current.retired = true;
    replace_state(store, &before, current, None).await
}

fn custody_protects_provider_credential(
    state: &SessionCredentialState,
    id: &str,
    now: SystemTime,
) -> bool {
    state_accounts_for_provider_credential(state, id)
        || state
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at > now)
}

impl AuthState {
    async fn reconcile_active_credential_page(
        &self,
        service: &Arc<layer2_core::SessionService>,
        budget: &AuthOperationBudget,
    ) -> Result<(), StoreError> {
        let mut epoch = self.active_credential_sweep_epoch.lock().await;
        let mut proposed = epoch.clone();
        if proposed.is_none() {
            if !budget.take(1) {
                return Ok(());
            }
            proposed = Some(SweepEpoch {
                cutoff: None,
                through_sequence: self.store.auth_sweep_high_water().await?,
                after: None,
            });
        }
        let current = proposed
            .clone()
            .expect("active epoch was initialized above");

        // One list call plus, per returned row: the reconciliation attempt,
        // isolated body read, session claim, and at most a sequence allocation
        // plus generation-CAS retirement write.
        let limit = budget.remaining().saturating_sub(1) / 5;
        if limit == 0 {
            *epoch = proposed;
            return Ok(());
        }
        if !budget.take(1) {
            *epoch = proposed;
            return Ok(());
        }
        let cursors = self
            .store
            .active_credentials_for_reconciliation(
                current.after.as_ref(),
                current.through_sequence,
                limit,
            )
            .await?;
        if !budget.take(cursors.len()) {
            return Err(StoreError::Malformed(
                "credential reconciliation store exceeded its requested page limit".into(),
            ));
        }
        proposed = finish_sweep_page(&cursors, limit, &current, |cursor| Some(cursor.clone()));

        let mut first_error = None;
        for cursor in cursors {
            if !budget.take(1) {
                return Err(StoreError::Malformed(
                    "credential reconciliation read budget invariant was violated".into(),
                ));
            }
            let state = match self.store.credential(&cursor.session).await {
                Ok(Some(state)) if !state.retired => state,
                Ok(_) => continue,
                Err(error) => {
                    remember_pass_error(&mut first_error, error, "read active credential custody");
                    continue;
                }
            };
            if !budget.take(1) {
                return Err(StoreError::Malformed(
                    "credential reconciliation claim budget invariant was violated".into(),
                ));
            }
            match service.session_claims_credential(&state.session).await {
                Ok(true) => {}
                Ok(false) => {
                    if !budget.take(2) {
                        return Err(StoreError::Malformed(
                            "credential reconciliation retirement budget invariant was violated"
                                .into(),
                        ));
                    }
                    if let Err(error) = retire_reconciled_state_once(&self.store, state).await {
                        remember_pass_error(
                            &mut first_error,
                            error,
                            "retire unclaimed credential custody",
                        );
                    }
                }
                Err(error) => remember_pass_error(
                    &mut first_error,
                    error,
                    "read session claim for credential custody",
                ),
            }
        }
        *epoch = proposed;
        first_error.map_or(Ok(()), Err)
    }

    async fn reconcile_provider_orphans(
        &self,
        service: &Arc<layer2_core::SessionService>,
        issuer: &Arc<super::keys::KeyIssuer>,
        budget: &AuthOperationBudget,
    ) -> Result<usize, StoreError> {
        let mut sweep = self.orphan_reconciliation_sweep.lock().await;
        let mut proposed = sweep.clone();
        let mut first_error = None;
        let mut reclaimed = 0;

        while budget.remaining() > 0 {
            if proposed.current_owner.is_none() {
                if proposed.owners.is_none() {
                    if !budget.take(1) {
                        break;
                    }
                    proposed.owners = Some(SweepEpoch {
                        cutoff: None,
                        through_sequence: match self.store.auth_sweep_high_water().await {
                            Ok(through) => through,
                            Err(error) => {
                                remember_pass_error(
                                    &mut first_error,
                                    error,
                                    "find orphan-owner high water",
                                );
                                break;
                            }
                        },
                        after: None,
                    });
                }

                let owner_epoch = proposed
                    .owners
                    .clone()
                    .expect("an owner epoch was initialized above");
                if !budget.take(1) {
                    break;
                }
                let mut owners = match self
                    .store
                    .subjects_with_tokens(
                        owner_epoch.after.as_ref(),
                        owner_epoch.through_sequence,
                        1,
                    )
                    .await
                {
                    Ok(owners) => owners,
                    Err(error) => {
                        remember_pass_error(&mut first_error, error, "read next orphan owner");
                        break;
                    }
                };
                let owners_after_completion =
                    finish_sweep_page(&owners, 1, &owner_epoch, |cursor| Some(cursor.clone()));
                let Some(owner) = owners.pop() else {
                    proposed.owners = owners_after_completion;
                    if proposed.owners.is_none() {
                        break;
                    }
                    continue;
                };
                if !budget.take(1) {
                    return Err(StoreError::Malformed(
                        "orphan-owner store exceeded its requested page limit".into(),
                    ));
                }
                proposed.current_owner = Some(OrphanOwnerSweep {
                    subject: owner.subject,
                    remaining_credentials: None,
                    owners_after_completion,
                });
            }

            let mut current = proposed
                .current_owner
                .clone()
                .expect("the current owner was initialized above");
            if !budget.take(1) {
                break;
            }
            let owner = match self.store.subject(&current.subject).await {
                Ok(Some(owner)) => owner,
                Ok(None) => {
                    proposed.owners = current.owners_after_completion;
                    proposed.current_owner = None;
                    if proposed.owners.is_none() {
                        break;
                    }
                    continue;
                }
                Err(error) => {
                    remember_pass_error(&mut first_error, error, "reload orphan owner token");
                    proposed.owners = current.owners_after_completion;
                    proposed.current_owner = None;
                    if proposed.owners.is_none() {
                        break;
                    }
                    continue;
                }
            };
            let Some(tokens) = owner.tokens else {
                proposed.owners = current.owners_after_completion;
                proposed.current_owner = None;
                if proposed.owners.is_none() {
                    break;
                }
                continue;
            };
            let Some(access_token) = tokens.access_token else {
                proposed.owners = current.owners_after_completion;
                proposed.current_owner = None;
                if proposed.owners.is_none() {
                    break;
                }
                continue;
            };
            let token = access_token.into_inner();
            if current.remaining_credentials.is_none() {
                if !budget.take(1) {
                    proposed.current_owner = Some(current);
                    break;
                }
                let mut minted = match issuer.minted_here(&token).await {
                    Ok(minted) => minted,
                    Err(_) => {
                        remember_pass_error(
                            &mut first_error,
                            StoreError::Unavailable(
                                "provider credential listing failed during orphan reconciliation"
                                    .into(),
                            ),
                            "list provider credentials for owner",
                        );
                        proposed.owners = current.owners_after_completion;
                        proposed.current_owner = None;
                        if proposed.owners.is_none() {
                            break;
                        }
                        continue;
                    }
                };
                minted.sort_by(|left, right| left.0.cmp(&right.0));
                minted.dedup_by(|left, right| left.0 == right.0);
                current.remaining_credentials = Some(minted.into());
            }

            while let Some((id, session)) = current
                .remaining_credentials
                .as_mut()
                .expect("the provider snapshot was initialized above")
                .pop_front()
            {
                // A candidate costs one reconciliation attempt and can require
                // one custody read, one session claim, and one provider DELETE.
                // Reserve all four before removing it from the durable cursor.
                if !budget.take(4) {
                    current
                        .remaining_credentials
                        .as_mut()
                        .expect("the provider snapshot remains initialized")
                        .push_front((id, session));
                    break;
                }

                let custody = match self.store.credential(&session).await {
                    Ok(custody) => custody,
                    Err(error) => {
                        remember_pass_error(
                            &mut first_error,
                            error,
                            "read provider candidate custody",
                        );
                        continue;
                    }
                };
                match custody {
                    Some(state)
                        if custody_protects_provider_credential(&state, &id, SystemTime::now()) => {
                    }
                    Some(_) => {
                        if issuer.revoke(&id, &token).await {
                            reclaimed += 1;
                        }
                    }
                    None => match service.session_claims_credential(&session).await {
                        Ok(true) => {}
                        Ok(false) => {
                            if issuer.revoke(&id, &token).await {
                                reclaimed += 1;
                            }
                        }
                        Err(error) => remember_pass_error(
                            &mut first_error,
                            error,
                            "read provider candidate session claim",
                        ),
                    },
                }
            }

            if current
                .remaining_credentials
                .as_ref()
                .is_some_and(std::collections::VecDeque::is_empty)
            {
                proposed.owners = current.owners_after_completion;
                proposed.current_owner = None;
                if proposed.owners.is_none() {
                    break;
                }
            } else {
                proposed.current_owner = Some(current);
                break;
            }
        }

        *sweep = proposed;
        first_error.map_or(Ok(reclaimed), Err)
    }

    #[cfg(test)]
    pub(super) async fn mint_session_credential(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        replacement: bool,
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential_inner(session, subject, services, None, replacement, None)
            .await
    }

    async fn mint_session_credential_for_request(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        caller_credential: Option<&CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential_inner(
            session,
            subject,
            services,
            caller_credential,
            false,
            None,
        )
        .await
    }

    pub(super) async fn mint_session_credential_with_budget(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        replacement: bool,
        budget: AuthOperationBudget,
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential_inner(
            session,
            subject,
            services,
            None,
            replacement,
            Some(budget),
        )
        .await
    }

    async fn mint_session_credential_inner(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        caller_credential: Option<&CallerCredential>,
        replacement: bool,
        budget: Option<AuthOperationBudget>,
    ) -> Result<Option<String>, StoreError> {
        let fingerprint = scope_fingerprint(services);
        let Some(issuer) = self.issuer.read().await.clone() else {
            loop {
                let current = ensure_state(&self.store, session, budget.as_ref()).await?;
                if current.retired {
                    return Ok(None);
                }
                if let Some(pending) = current.pending.as_ref() {
                    let Some(held) = current.current.as_ref() else {
                        return Ok(None);
                    };
                    validate_credential_grant(held, subject, &fingerprint)?;
                    validate_credential_grant(pending, subject, &fingerprint)?;
                    let effective = if replacement { pending } else { held };
                    return Ok(Some(effective.value.expose().to_owned()));
                }
                if let Some(held) = current.current.as_ref() {
                    validate_credential_grant(held, subject, &fingerprint)?;
                    if replacement {
                        // Without an issuer there is no way to create a new
                        // replacement. A recorded pending value is handled above.
                        return Ok(None);
                    }
                    return Ok(Some(held.value.expose().to_owned()));
                }
                if replacement {
                    return Ok(None);
                }
                // A scoped mint is still in flight but cannot be completed
                // without its issuer. Falling back would silently widen the
                // session while its frozen reach reports the narrower grant.
                if current.lease.is_some() {
                    return Ok(None);
                }
                if let Some(credential) = current.request_bound_credential {
                    return Ok(Some(credential.into_inner()));
                }
                if let Some(credential) = caller_credential {
                    if !credential.is_request_bound() {
                        return Ok(Some(credential.as_ref().to_owned()));
                    }
                    let mut next = current.clone();
                    next.request_bound_credential =
                        Some(SecretValue::new(credential.as_ref().to_owned()));
                    if replace_state(&self.store, &current, next, budget.as_ref()).await? {
                        return Ok(Some(credential.as_ref().to_owned()));
                    }
                    continue;
                }
                take_operation(budget.as_ref(), 1, "subject access-token read")?;
                return self.access_token(subject).await;
            }
        };
        let pinned_request_credential = if caller_credential
            .is_some_and(CallerCredential::is_request_bound)
        {
            loop {
                let current = ensure_state(&self.store, session, budget.as_ref()).await?;
                if current.retired {
                    return Ok(None);
                }
                if let Some(credential) = current.request_bound_credential {
                    break Some(credential.into_inner());
                }
                let credential = caller_credential.expect("request-bound credential is present");
                let mut next = current.clone();
                next.request_bound_credential =
                    Some(SecretValue::new(credential.as_ref().to_owned()));
                if replace_state(&self.store, &current, next, budget.as_ref()).await? {
                    break Some(credential.as_ref().to_owned());
                }
            }
        } else {
            None
        };
        let mut revocation_credential = pinned_request_credential.clone();
        let mut owner_token = pinned_request_credential
            .or_else(|| caller_credential.map(|credential| credential.as_ref().to_owned()));

        let (holder, fence) = loop {
            let current = ensure_state(&self.store, session, budget.as_ref()).await?;
            if current.retired {
                return Ok(None);
            }
            if let Some(pending) = current.pending.as_ref() {
                let Some(held) = current.current.as_ref() else {
                    return Ok(None);
                };
                validate_credential_grant(held, subject, &fingerprint)?;
                validate_credential_grant(pending, subject, &fingerprint)?;
                let effective = if replacement { pending } else { held };
                return Ok(Some(effective.value.expose().to_owned()));
            }
            if replacement {
                let Some(held) = current.current.as_ref() else {
                    return Ok(None);
                };
                validate_credential_grant(held, subject, &fingerprint)?;
            } else if let Some(held) = current.current.as_ref() {
                validate_credential_grant(held, subject, &fingerprint)?;
                return Ok(Some(held.value.expose().to_owned()));
            }

            if current
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > SystemTime::now())
            {
                if budget.is_some() {
                    return Ok(None);
                }
                tokio::time::sleep(LEASE_POLL_INTERVAL).await;
                continue;
            }

            if owner_token.is_none() {
                if let Some(credential) = current.request_bound_credential.as_ref() {
                    let credential = credential.expose().to_owned();
                    revocation_credential = Some(credential.clone());
                    owner_token = Some(credential);
                }
                if owner_token.is_none() {
                    take_operation(budget.as_ref(), 1, "credential owner token read")?;
                    owner_token = self.access_token(subject).await?;
                }
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
            if replace_state(&self.store, &current, next, budget.as_ref()).await? {
                break (holder, fence);
            }
        };

        let task = tokio::spawn(finish_mint(
            Arc::clone(&self.store),
            issuer,
            session.clone(),
            subject.clone(),
            owner_token.expect("a mint lease is acquired only with an owner token"),
            revocation_credential,
            services.to_vec(),
            fingerprint,
            holder,
            fence,
            replacement,
            budget,
        ));
        task.await.map_err(|error| {
            StoreError::Unavailable(format!("credential mint task failed: {error}"))
        })?
    }

    #[cfg(test)]
    pub(super) async fn promote_pending_credential(
        &self,
        session: &SessionId,
    ) -> Result<bool, StoreError> {
        self.promote_pending_credential_inner(session, None).await
    }

    pub(super) async fn promote_pending_credential_with_budget(
        &self,
        session: &SessionId,
        budget: &AuthOperationBudget,
    ) -> Result<bool, StoreError> {
        self.promote_pending_credential_inner(session, Some(budget))
            .await
    }

    async fn promote_pending_credential_inner(
        &self,
        session: &SessionId,
        budget: Option<&AuthOperationBudget>,
    ) -> Result<bool, StoreError> {
        loop {
            take_operation(budget, 1, "pending credential promotion read")?;
            let Some(mut current) = self.store.credential(session).await? else {
                return Ok(false);
            };
            let Some(replacement) = current.pending.take() else {
                return Ok(false);
            };
            let before = current.clone();
            if current.retired {
                queue_debt(&mut current, &replacement);
                if replace_state(&self.store, &before, current, budget).await? {
                    return Ok(false);
                }
                continue;
            }
            if let Some(displaced) = current.current.replace(replacement) {
                queue_debt(&mut current, &displaced);
            }
            if replace_state(&self.store, &before, current, budget).await? {
                return Ok(true);
            }
        }
    }

    pub(super) async fn revoke_credential_debt(
        &self,
        budget: &AuthOperationBudget,
    ) -> Result<usize, StoreError> {
        let _sweep = self.credential_sweep.lock().await;
        let Some(issuer) = self.issuer.read().await.clone() else {
            return Ok(0);
        };
        let mut epoch = self.revoke_debt_sweep_epoch.lock().await;
        let mut proposed = epoch.clone();
        if proposed.is_none() {
            if !budget.take(1) {
                return Ok(0);
            }
            proposed = Some(SweepEpoch {
                cutoff: None,
                through_sequence: self.store.auth_sweep_high_water().await?,
                after: None,
            });
        }
        let mut taken = 0;
        let mut owner_tokens: HashMap<SubjectId, Option<String>> = HashMap::new();
        let mut first_error = None;
        while let Some(current) = proposed.clone() {
            // Mongo's indexed traversal performs at most two reads to locate
            // one debt. The remaining six units cover the returned-item
            // attempt, owner read, provider DELETE, custody read, and the
            // sequence allocation plus CAS write used to settle it.
            if !budget.take(8) {
                break;
            }
            let mut page = match self
                .store
                .revoke_debts(current.after.as_ref(), current.through_sequence, 1)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    remember_pass_error(
                        &mut first_error,
                        error,
                        "read next credential revoke debt",
                    );
                    break;
                }
            };
            if page.len() > 1 {
                return Err(StoreError::Malformed(
                    "credential revoke-debt store exceeded its requested page limit".into(),
                ));
            }
            let next_epoch = finish_sweep_page(&page, 1, &current, revoke_debt_work_position);
            let Some(item) = page.pop() else {
                proposed = None;
                break;
            };
            let work = match item.state {
                Ok(work) => work,
                Err(error) => {
                    remember_pass_error(
                        &mut first_error,
                        error,
                        "read credential revoke-debt item",
                    );
                    proposed = next_epoch;
                    continue;
                }
            };
            let debt = work.debt;
            let token = if let Some(credential) = debt.revocation_credential.as_ref() {
                Some(credential.expose().to_owned())
            } else if let Some(cached) = owner_tokens.get(&debt.owner) {
                cached.clone()
            } else {
                match self.access_token(&debt.owner).await {
                    Ok(loaded) => {
                        owner_tokens.insert(debt.owner.clone(), loaded.clone());
                        loaded
                    }
                    Err(error) => {
                        remember_pass_error(
                            &mut first_error,
                            error,
                            "read revoke-debt owner token",
                        );
                        owner_tokens.insert(debt.owner.clone(), None);
                        None
                    }
                }
            };
            let Some(token) = token else {
                proposed = next_epoch;
                continue;
            };
            if !issuer.revoke(&debt.id, &token).await {
                proposed = next_epoch;
                continue;
            }

            let mut current = match self.store.credential(&work.session).await {
                Ok(Some(current)) => current,
                Ok(None) => {
                    proposed = next_epoch;
                    continue;
                }
                Err(error) => {
                    remember_pass_error(&mut first_error, error, "read revoked credential custody");
                    proposed = next_epoch;
                    continue;
                }
            };
            let before = current.clone();
            let old_len = current.revoke_debt.len();
            current.revoke_debt.retain(|queued| queued.id != debt.id);
            if current.revoke_debt.len() == old_len {
                proposed = next_epoch;
                continue;
            }
            // A legacy terminal debt can borrow the top-level request bearer
            // during projection. Keep it while other debts still need that
            // fallback, then release it so terminal retention can begin.
            if current.retired
                && current.current.is_none()
                && current.pending.is_none()
                && current.revoke_debt.is_empty()
                && current.lease.is_none()
            {
                current.request_bound_credential = None;
            }
            match replace_state(&self.store, &before, current, None).await {
                Ok(true) => taken += 1,
                Ok(false) => {}
                Err(error) => remember_pass_error(
                    &mut first_error,
                    error,
                    "settle revoked credential custody",
                ),
            }
            proposed = next_epoch;
        }
        // The page position is published only after every item was attempted.
        // Failed items recur after this finite epoch wraps, while later healthy
        // items are never trapped behind a deterministic first-row failure.
        *epoch = proposed;
        drop(epoch);
        first_error.map_or(Ok(taken), Err)
    }

    #[cfg(test)]
    pub(super) async fn reclaim_unrecorded_credentials(
        &self,
        service: &Arc<layer2_core::SessionService>,
    ) -> Result<usize, StoreError> {
        let active_budget = AuthOperationBudget::standalone(AUTH_SWEEP_TWO_LANE_BATCH_SIZE);
        let provider_budget = AuthOperationBudget::standalone(AUTH_SWEEP_TWO_LANE_BATCH_SIZE);
        self.reclaim_unrecorded_credentials_with_budgets(service, &active_budget, &provider_budget)
            .await
    }

    pub(super) async fn reclaim_unrecorded_credentials_with_budgets(
        &self,
        service: &Arc<layer2_core::SessionService>,
        active_budget: &AuthOperationBudget,
        provider_budget: &AuthOperationBudget,
    ) -> Result<usize, StoreError> {
        let _sweep = self.credential_sweep.lock().await;
        let active = self
            .reconcile_active_credential_page(service, active_budget)
            .await;
        let Some(issuer) = self.issuer.read().await.clone() else {
            return active.map(|()| 0);
        };
        let provider = self
            .reconcile_provider_orphans(service, &issuer, provider_budget)
            .await;

        match (active, provider) {
            (Ok(()), Ok(reclaimed)) => Ok(reclaimed),
            (Err(error), Ok(_)) | (Ok(()), Err(error)) => Err(error),
            (Err(first), Err(second)) => {
                tracing::warn!(error = %second, "provider orphan lane also failed this pass");
                Err(first)
            }
        }
    }
}

#[async_trait::async_trait]
impl layer2_core::CredentialSource for AuthState {
    async fn credential_for(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[ServiceId],
        caller_credential: Option<&CallerCredential>,
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential_for_request(session, subject, services, caller_credential)
            .await
    }

    async fn tracks_subject_credential(&self, session: &SessionId) -> Result<bool, StoreError> {
        if self.issuer.read().await.is_some() {
            return Ok(false);
        }
        Ok(self.store.credential(session).await?.is_none_or(|state| {
            !state.retired
                && state.request_bound_credential.is_none()
                && state.current.is_none()
                && state.pending.is_none()
                && state.lease.is_none()
        }))
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

#[cfg(test)]
pub(crate) use reclaim_tests::empty_session_service;
