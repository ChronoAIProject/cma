use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, RwLock,
};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify};

use super::*;
use crate::{
    Authorization, Caller, Capability, Page, PageCursor, PageRequest, SessionId, SubjectId,
};

const NOW: &str = "2026-08-29T00:00:00Z";
const EARLIER: &str = "2026-08-28T23:59:59Z";
const LATER: &str = "2026-08-29T00:02:00Z";
const GRANT_EXPIRY: &str = "2026-08-29T01:00:00Z";
const POLICY_EXPIRY: &str = "2026-08-30T00:00:00Z";
const PROVIDER: &str = "provider-a";
const POLICY_KEY: &str = "provider-a:least-privilege-v1";
const ALTERNATE_POLICY_KEY: &str = "provider-a:least-privilege-v2";
const MODEL: &str = "model-a";
const RESOURCE: &str = "responses";
const SECRET: &str = "fixture-provider-credential";

struct MemoryState {
    authoritative_now: String,
    gate_fence: SponsorshipGateFence,
    kill_switch_engagement_id: Option<String>,
    launches: BTreeMap<String, SponsoredLaunchAuthority>,
    credentials: BTreeMap<String, (SponsoredCredentialRecord, Option<String>)>,
    credential_provider_aliases: BTreeMap<String, BTreeSet<String>>,
    credential_registrations: Vec<MemoryCredentialRegistration>,
    credential_operations:
        BTreeMap<(String, ProviderOperationKind, String), CompletedProviderOperation>,
    pending_credential_secrets: BTreeMap<String, String>,
    policies: BTreeMap<String, SponsorshipPolicy>,
    policy_usage: BTreeMap<String, SponsorshipPolicyUsage>,
    acceptances: BTreeMap<String, SponsoredGrantAcceptance>,
    acceptance_aliases: BTreeMap<String, BTreeSet<String>>,
    grants: BTreeMap<String, SponsoredGrant>,
    grants_by_launch: BTreeMap<String, String>,
    grant_receipts: BTreeMap<String, ProviderGrantReceipt>,
    activations: BTreeMap<String, GrantActivationIntent>,
    reservations: BTreeMap<String, SponsoredUsageReservation>,
    reservation_aliases: BTreeMap<String, BTreeSet<String>>,
    reservation_policy: BTreeMap<String, String>,
    reservation_times: BTreeMap<String, Vec<u64>>,
    audits: Vec<SponsoredAuditEvent>,
}

impl Default for MemoryState {
    fn default() -> Self {
        Self {
            authoritative_now: NOW.into(),
            gate_fence: SponsorshipGateFence {
                configuration_digest: digest(b"unconfigured-gate"),
                epoch: 1,
                kill_switch_engaged: true,
            },
            kill_switch_engagement_id: None,
            launches: BTreeMap::new(),
            credentials: BTreeMap::new(),
            credential_provider_aliases: BTreeMap::new(),
            credential_registrations: Vec::new(),
            credential_operations: BTreeMap::new(),
            pending_credential_secrets: BTreeMap::new(),
            policies: BTreeMap::new(),
            policy_usage: BTreeMap::new(),
            acceptances: BTreeMap::new(),
            acceptance_aliases: BTreeMap::new(),
            grants: BTreeMap::new(),
            grants_by_launch: BTreeMap::new(),
            grant_receipts: BTreeMap::new(),
            activations: BTreeMap::new(),
            reservations: BTreeMap::new(),
            reservation_aliases: BTreeMap::new(),
            reservation_policy: BTreeMap::new(),
            reservation_times: BTreeMap::new(),
            audits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct MemoryCredentialRegistration {
    request_aliases: BTreeSet<String>,
    command_aliases: BTreeSet<String>,
    credential_id: String,
}

fn pseudonym_aliases(candidates: &SponsorshipOperationalPseudonymSet) -> BTreeSet<String> {
    candidates
        .iter()
        .map(|candidate| candidate.as_str().to_string())
        .collect()
}

fn aliases_match(
    stored: &BTreeSet<String>,
    candidates: &SponsorshipOperationalPseudonymSet,
) -> bool {
    candidates
        .iter()
        .any(|candidate| stored.contains(candidate.as_str()))
}

fn credential_retirement_covers_activation(
    activation: &GrantActivationIntent,
    policy: &SponsorshipPolicy,
    credential: &SponsoredCredentialRecord,
) -> bool {
    let confirmed_revocation =
        credential
            .last_completed_operation
            .as_ref()
            .is_some_and(|operation| {
                operation.kind == ProviderOperationKind::RevokeCredential
                    && operation.outcome == CompletedProviderOperationOutcome::Confirmed
            });
    credential.state == SponsoredCredentialState::Revoked
        && credential.material_state == SponsoredCredentialMaterialState::Retired
        && credential.revoked_at.is_some()
        && confirmed_revocation
        && policy.id == activation.policy_id
        && policy.generation >= activation.policy_generation
        && policy.credential_id == credential.id
        && credential.generation > activation.credential_generation
        && credential.owner == policy.owner
        && credential.authority.provider == policy.provider_policy.provider
        && credential.authority.provider_policy_key == policy.provider_policy.provider_policy_key
        && activation.allocation.quota == policy.per_launch_quota
        && activation.allocation.spend_micros == policy.per_launch_budget_micros
}

#[derive(Default)]
struct MemoryStore {
    state: Mutex<MemoryState>,
    provider_receipt_reads: AtomicUsize,
    policy_replace_stale_failures: AtomicUsize,
    policy_replace_attempts: AtomicUsize,
}

impl MemoryStore {
    async fn set_authoritative_now(&self, now: &str) {
        assert!(crate::seconds_from_rfc3339(now).is_some());
        self.state.lock().await.authoritative_now = now.to_string();
    }

    async fn configure_gate(&self, gate: &SponsorshipGate) {
        let mut state = self.state.lock().await;
        state.gate_fence = SponsorshipGateFence {
            configuration_digest: gate.configuration_digest().unwrap(),
            epoch: 1,
            kill_switch_engaged: gate.kill_switch_engaged,
        };
        state.kill_switch_engagement_id = None;
    }

    async fn put_launch(&self, launch: SponsoredLaunchAuthority) {
        launch.validate().unwrap();
        self.state
            .lock()
            .await
            .launches
            .insert(launch.launch_id.0.clone(), launch);
    }

    async fn update_launch_state(
        &self,
        launch_id: &LaunchId,
        state: SponsoredLaunchState,
        session_id: Option<SessionId>,
    ) {
        let mut memory = self.state.lock().await;
        let launch = memory.launches.get_mut(&launch_id.0).unwrap();
        launch.state = state;
        launch.session_id = session_id;
        launch.generation += 1;
        launch.digest = launch.calculate_digest().unwrap();
    }

    async fn snapshot(&self) -> MemorySnapshot {
        let state = self.state.lock().await;
        MemorySnapshot {
            grants: state.grants.values().cloned().collect(),
            reservations: state.reservations.values().cloned().collect(),
            audits: state.audits.clone(),
            credentials: state
                .credentials
                .values()
                .map(|(record, _)| record.clone())
                .collect(),
        }
    }
}

struct MemorySnapshot {
    grants: Vec<SponsoredGrant>,
    reservations: Vec<SponsoredUsageReservation>,
    audits: Vec<SponsoredAuditEvent>,
    credentials: Vec<SponsoredCredentialRecord>,
}

fn newest_memory_page<T>(
    mut items: Vec<T>,
    page: &PageRequest,
    key: impl Fn(&T) -> (&str, &str),
) -> Page<T> {
    items.sort_by(|left, right| key(right).cmp(&key(left)));
    if let Some(cursor) = page.cursor() {
        items.retain(|item| key(item) < (cursor.created_at.as_str(), cursor.id.as_str()));
    }
    let limit = page.limit() as usize;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next = has_more.then(|| {
        let (created_at, id) = key(items.last().expect("a full page has a final row"));
        PageCursor::new(created_at, id)
    });
    Page { items, next }
}

fn recovery_memory_page<T>(
    mut items: Vec<T>,
    page: &SponsorshipRecoveryPageRequest,
    domain: &str,
    key: impl Fn(&T) -> (&str, &str),
) -> Result<SponsorshipRecoveryPage<T>, SponsorshipStoreError> {
    #[derive(serde::Deserialize, serde::Serialize)]
    #[serde(deny_unknown_fields)]
    struct Cursor {
        version: u8,
        domain: String,
        ordered_at: String,
        id: String,
    }

    items.sort_by(|left, right| key(left).cmp(&key(right)));
    if let Some(cursor) = page.cursor() {
        if cursor.is_empty() || cursor.len() > 2_048 {
            return Err(SponsorshipStoreError::Malformed(
                "recovery cursor is not usable".into(),
            ));
        }
        let cursor = serde_json::from_str::<Cursor>(cursor).map_err(|_| {
            SponsorshipStoreError::Malformed("recovery cursor is not usable".into())
        })?;
        if cursor.version != 1
            || cursor.domain != domain
            || crate::seconds_from_rfc3339(&cursor.ordered_at).is_none()
            || cursor.id.is_empty()
            || cursor.id.len() > 512
        {
            return Err(SponsorshipStoreError::Malformed(
                "recovery cursor is not usable".into(),
            ));
        }
        items.retain(|item| key(item) > (cursor.ordered_at.as_str(), cursor.id.as_str()));
    }
    let limit = page.limit() as usize;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next = has_more
        .then(|| {
            let (ordered_at, id) = key(items.last().expect("a full recovery page has a final row"));
            serde_json::to_string(&Cursor {
                version: 1,
                domain: domain.to_string(),
                ordered_at: ordered_at.to_string(),
                id: id.to_string(),
            })
            .map_err(|_| {
                SponsorshipStoreError::Malformed("recovery cursor cannot be encoded".into())
            })
        })
        .transpose()?;
    if next.as_ref().is_some_and(|cursor| cursor.len() > 2_048) {
        return Err(SponsorshipStoreError::Malformed(
            "recovery cursor exceeds its size bound".into(),
        ));
    }
    Ok(SponsorshipRecoveryPage { items, next })
}

#[async_trait]
impl SponsorshipStore for MemoryStore {
    async fn gate_fence(&self) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
        Ok(self.state.lock().await.gate_fence.clone())
    }

    async fn engage_kill_switch(
        &self,
        expected_epoch: u64,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        if state.gate_fence.kill_switch_engaged {
            return Ok(state.gate_fence.clone());
        }
        if state.gate_fence.epoch != expected_epoch {
            return Err(SponsorshipStoreError::GateChanged);
        }
        state.gate_fence.epoch += 1;
        state.gate_fence.kill_switch_engaged = true;
        state.kill_switch_engagement_id = Some(audit.event_id.clone());
        state.audits.push(audit);
        let authoritative_now = state.authoritative_now.clone();
        let terminalizable = state
            .activations
            .values()
            .filter(|intent| {
                matches!(
                    intent.state,
                    GrantActivationState::AwaitingBilling | GrantActivationState::AwaitingProvider
                )
            })
            .map(|intent| intent.launch_id.0.clone())
            .collect::<Vec<_>>();
        for launch_id in terminalizable {
            let mut intent = state.activations.remove(&launch_id).unwrap();
            if intent.capacity_held {
                release_activation_allocation(&mut state, &intent)?;
            }
            intent.state = GrantActivationState::Denied;
            intent.denial = Some(GrantActivationDenial::AuthorityInvalidated);
            intent.capacity_held = false;
            intent.lease = None;
            intent.revision = intent.revision.checked_add(1).ok_or_else(|| {
                SponsorshipStoreError::Malformed("activation revision exhausted".into())
            })?;
            intent.updated_at = authoritative_now.clone();
            intent
                .validate()
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
            state.activations.insert(launch_id, intent);
        }
        Ok(state.gate_fence.clone())
    }

    async fn launch_authority(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredLaunchAuthority>, SponsorshipStoreError> {
        Ok(self.state.lock().await.launches.get(&launch_id.0).cloned())
    }

    async fn credential_registration(
        &self,
        identity: &SponsoredCredentialRegistrationIdentity,
    ) -> Result<Option<SponsoredCredentialRecord>, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let matches = state
            .credential_registrations
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| {
                aliases_match(&binding.request_aliases, &identity.request).then_some(index)
            })
            .collect::<Vec<_>>();
        let Some(&index) = matches.first() else {
            return Ok(None);
        };
        if matches.len() != 1 {
            return Err(SponsorshipStoreError::Malformed(
                "credential registration candidates resolve multiple durable bindings".into(),
            ));
        }
        if !aliases_match(
            &state.credential_registrations[index].command_aliases,
            &identity.command,
        ) {
            return Err(SponsorshipStoreError::Conflict);
        }
        state.credential_registrations[index].request_aliases =
            pseudonym_aliases(&identity.request);
        state.credential_registrations[index].command_aliases =
            pseudonym_aliases(&identity.command);
        let credential_id = state.credential_registrations[index].credential_id.clone();
        state
            .credentials
            .get(&credential_id)
            .map(|(record, _)| record.clone())
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "credential registration points to no record".into(),
                )
            })
            .map(Some)
    }

    async fn insert_credential(
        &self,
        identity: &SponsoredCredentialRegistrationIdentity,
        provider_identity: &SponsorshipOperationalPseudonymSet,
        record: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if record.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        let mut state = self.state.lock().await;
        let registration_matches = state
            .credential_registrations
            .iter()
            .enumerate()
            .filter_map(|(index, binding)| {
                aliases_match(&binding.request_aliases, &identity.request).then_some(index)
            })
            .collect::<Vec<_>>();
        if registration_matches.len() > 1 {
            return Err(SponsorshipStoreError::Malformed(
                "credential registration candidates resolve multiple durable bindings".into(),
            ));
        }
        if let Some(&index) = registration_matches.first() {
            if !aliases_match(
                &state.credential_registrations[index].command_aliases,
                &identity.command,
            ) {
                return Err(SponsorshipStoreError::Conflict);
            }
            state.credential_registrations[index].request_aliases =
                pseudonym_aliases(&identity.request);
            state.credential_registrations[index].command_aliases =
                pseudonym_aliases(&identity.command);
            let id = state.credential_registrations[index].credential_id.clone();
            let stored = state
                .credentials
                .get(&id)
                .map(|(stored, _)| stored)
                .ok_or_else(|| {
                    SponsorshipStoreError::Malformed(
                        "credential registration points to no record".into(),
                    )
                })?;
            if credential_registration_bindings_match(stored, record) {
                return Ok(stored.clone());
            }
            return Err(SponsorshipStoreError::Conflict);
        }
        let provider_matches = state
            .credential_provider_aliases
            .iter()
            .filter_map(|(id, aliases)| {
                let provider_matches = state.credentials.get(id).is_some_and(|(stored, _)| {
                    stored.authority.provider == record.authority.provider
                });
                (provider_matches && aliases_match(aliases, provider_identity))
                    .then_some(id.clone())
            })
            .collect::<Vec<_>>();
        if provider_matches.len() > 1 {
            return Err(SponsorshipStoreError::Malformed(
                "provider credential candidates resolve multiple durable lineages".into(),
            ));
        }
        if let Some(id) = provider_matches.first() {
            let (stored, stored_secret) = state
                .credentials
                .get(id)
                .expect("provider alias map only contains credential ids");
            if !credential_registration_bindings_match(stored, record)
                || stored_secret.as_deref() != Some(secret.expose())
            {
                return Err(SponsorshipStoreError::Conflict);
            }
            let id = id.clone();
            let stored = stored.clone();
            state
                .credential_provider_aliases
                .insert(id.clone(), pseudonym_aliases(provider_identity));
            state
                .credential_registrations
                .push(MemoryCredentialRegistration {
                    request_aliases: pseudonym_aliases(&identity.request),
                    command_aliases: pseudonym_aliases(&identity.command),
                    credential_id: id,
                });
            return Ok(stored);
        }
        match state.credentials.get(&record.id.0) {
            Some((stored, Some(stored_secret)))
                if stored == record && stored_secret == secret.expose() =>
            {
                let stored = stored.clone();
                state
                    .credential_provider_aliases
                    .insert(record.id.0.clone(), pseudonym_aliases(provider_identity));
                state
                    .credential_registrations
                    .push(MemoryCredentialRegistration {
                        request_aliases: pseudonym_aliases(&identity.request),
                        command_aliases: pseudonym_aliases(&identity.command),
                        credential_id: record.id.0.clone(),
                    });
                Ok(stored)
            }
            Some(_) => Err(SponsorshipStoreError::Conflict),
            None => {
                state
                    .credential_provider_aliases
                    .insert(record.id.0.clone(), pseudonym_aliases(provider_identity));
                state
                    .credential_registrations
                    .push(MemoryCredentialRegistration {
                        request_aliases: pseudonym_aliases(&identity.request),
                        command_aliases: pseudonym_aliases(&identity.command),
                        credential_id: record.id.0.clone(),
                    });
                state.credentials.insert(
                    record.id.0.clone(),
                    (record.clone(), Some(secret.expose().to_string())),
                );
                state.audits.push(audit);
                Ok(record.clone())
            }
        }
    }

    async fn credential(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<StoredSponsoredCredential>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        state
            .credentials
            .get(&id.0)
            .map(|(record, secret)| {
                let secret = secret
                    .as_ref()
                    .map(|secret| ProviderCredentialSecret::new(secret.clone()))
                    .transpose()
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                let stored = StoredSponsoredCredential {
                    record: record.clone(),
                    secret,
                };
                stored.validate()?;
                Ok(stored)
            })
            .transpose()
    }

    async fn credential_record(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<SponsoredCredentialRecord>, SponsorshipStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .credentials
            .get(&id.0)
            .map(|(record, _)| record.clone()))
    }

    async fn credentials_for_owner(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredCredentialRecord>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        Ok(newest_memory_page(
            state
                .credentials
                .values()
                .map(|(record, _)| record)
                .filter(|record| record.owner.controller() == owner)
                .cloned()
                .collect(),
            page,
            |record| (&record.created_at, &record.id.0),
        ))
    }

    async fn completed_credential_operation(
        &self,
        id: &SponsoredCredentialId,
        kind: ProviderOperationKind,
        client_request_identity: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<CompletedProviderOperation>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        let matches = state
            .credential_operations
            .iter()
            .filter_map(|((credential_id, operation_kind, request_id), operation)| {
                (credential_id == &id.0
                    && operation_kind == &kind
                    && client_request_identity
                        .iter()
                        .any(|candidate| candidate.as_str() == request_id))
                .then_some(operation.clone())
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(SponsorshipStoreError::Malformed(
                "credential operation candidates resolve multiple durable operations".into(),
            ));
        }
        Ok(matches.into_iter().next())
    }

    async fn replace_credential(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        secret: Option<&ProviderCredentialSecret>,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let (stored, stored_secret) = state
            .credentials
            .get_mut(&record.id.0)
            .ok_or(SponsorshipStoreError::Conflict)?;
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if record.material_state != stored.material_state
            || record.material_state != SponsoredCredentialMaterialState::Retained
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        if stored.generation != expected_generation {
            if stored == record {
                return Ok(stored.clone());
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        *stored = record.clone();
        if let Some(secret) = secret {
            *stored_secret = Some(secret.expose().to_string());
        }
        state.audits.push(audit);
        Ok(record.clone())
    }

    async fn stage_credential_rotation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        replacement: &ProviderCredentialSecret,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if record.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        let pending = record
            .pending_operation
            .as_ref()
            .filter(|operation| operation.kind == ProviderOperationKind::RotateCredential)
            .ok_or(SponsorshipStoreError::Conflict)?;
        let operation_key = (
            record.id.0.clone(),
            pending.kind,
            pending.client_request_id_digest.clone(),
        );
        if state.credential_operations.contains_key(&operation_key) {
            return Err(SponsorshipStoreError::Conflict);
        }
        let stored = &state
            .credentials
            .get(&record.id.0)
            .ok_or(SponsorshipStoreError::Conflict)?
            .0;
        if stored.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        if stored.generation != expected_generation {
            if stored == record {
                return Ok(stored.clone());
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        state
            .pending_credential_secrets
            .insert(record.id.0.clone(), replacement.expose().to_string());
        state
            .credentials
            .get_mut(&record.id.0)
            .expect("credential exists")
            .0 = record.clone();
        state.audits.push(audit);
        Ok(record.clone())
    }

    async fn pending_credential_rotation_secret(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<ProviderCredentialSecret>, SponsorshipStoreError> {
        self.state
            .lock()
            .await
            .pending_credential_secrets
            .get(&id.0)
            .cloned()
            .map(ProviderCredentialSecret::new)
            .transpose()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))
    }

    async fn complete_credential_rotation(
        &self,
        expected_generation: u64,
        provider_identity: &SponsorshipOperationalPseudonymSet,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if record.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        let current = &state
            .credentials
            .get(&record.id.0)
            .ok_or(SponsorshipStoreError::Conflict)?
            .0;
        if current.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        if current.generation != expected_generation {
            if current == record {
                return Ok(current.clone());
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        let completion = credential_operation_completion(
            current,
            record,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Confirmed,
        )?;
        let operation_key = credential_operation_key(record, &completion);
        if state
            .credential_operations
            .get(&operation_key)
            .is_some_and(|stored| stored != &completion)
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        if state
            .credential_provider_aliases
            .iter()
            .any(|(id, aliases)| {
                id != &record.id.0
                    && state.credentials.get(id).is_some_and(|(stored, _)| {
                        stored.authority.provider == record.authority.provider
                    })
                    && aliases_match(aliases, provider_identity)
            })
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        let replacement = state
            .pending_credential_secrets
            .remove(&record.id.0)
            .ok_or_else(|| SponsorshipStoreError::Malformed("pending secret missing".into()))?;
        state
            .credentials
            .insert(record.id.0.clone(), (record.clone(), Some(replacement)));
        state
            .credential_provider_aliases
            .insert(record.id.0.clone(), pseudonym_aliases(provider_identity));
        state
            .credential_operations
            .insert(operation_key, completion);
        state.audits.push(audit);
        Ok(record.clone())
    }

    async fn abort_credential_rotation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if record.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        let stored = &state
            .credentials
            .get(&record.id.0)
            .ok_or(SponsorshipStoreError::Conflict)?
            .0;
        if stored.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        if stored.generation != expected_generation {
            if stored == record {
                return Ok(stored.clone());
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        let completion = credential_operation_completion(
            stored,
            record,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Rejected,
        )?;
        let operation_key = credential_operation_key(record, &completion);
        if state
            .credential_operations
            .get(&operation_key)
            .is_some_and(|stored| stored != &completion)
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        state.pending_credential_secrets.remove(&record.id.0);
        state
            .credentials
            .get_mut(&record.id.0)
            .expect("credential exists")
            .0 = record.clone();
        state
            .credential_operations
            .insert(operation_key, completion);
        state.audits.push(audit);
        Ok(record.clone())
    }

    async fn complete_credential_revocation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let current = &state
            .credentials
            .get(&record.id.0)
            .ok_or(SponsorshipStoreError::Conflict)?
            .0;
        if current.generation != expected_generation {
            if current == record {
                return Ok(current.clone());
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        let completion = credential_operation_completion(
            current,
            record,
            ProviderOperationKind::RevokeCredential,
            CompletedProviderOperationOutcome::Confirmed,
        )?;
        let operation_key = credential_operation_key(record, &completion);
        if state
            .credential_operations
            .get(&operation_key)
            .is_some_and(|stored| stored != &completion)
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        if current.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipStoreError::Conflict);
        }
        if record.state != SponsoredCredentialState::Revoked
            || record.material_state != SponsoredCredentialMaterialState::Retired
            || record.pending_operation.is_some()
            || record.superseded_operation.is_some()
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        state.pending_credential_secrets.remove(&record.id.0);
        let (stored_record, stored_secret) = state
            .credentials
            .get_mut(&record.id.0)
            .expect("credential exists");
        *stored_record = record.clone();
        *stored_secret = None;
        state
            .credential_operations
            .insert(operation_key, completion);
        state.audits.push(audit);
        Ok(record.clone())
    }

    async fn insert_policy(
        &self,
        policy: &SponsorshipPolicy,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipPolicy, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        match state.policies.get(&policy.id.0) {
            Some(stored) if stored == policy => Ok(stored.clone()),
            Some(_) => Err(SponsorshipStoreError::Conflict),
            None => {
                state.policies.insert(policy.id.0.clone(), policy.clone());
                state.policy_usage.insert(
                    policy.id.0.clone(),
                    SponsorshipPolicyUsage {
                        consumed: SponsoredUsageSummary::default(),
                        reserved: SponsoredUsageSummary::default(),
                        active_launches: 0,
                    },
                );
                state.audits.push(audit);
                Ok(policy.clone())
            }
        }
    }

    async fn policy(
        &self,
        id: &SponsorshipPolicyId,
    ) -> Result<Option<SponsorshipPolicy>, SponsorshipStoreError> {
        Ok(self.state.lock().await.policies.get(&id.0).cloned())
    }

    async fn policies_for_owner(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<StoredSponsorshipPolicy>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        let policies = state
            .policies
            .values()
            .filter(|policy| policy.owner.controller() == owner)
            .map(|policy| StoredSponsorshipPolicy {
                policy: policy.clone(),
                usage: state
                    .policy_usage
                    .get(&policy.id.0)
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect();
        Ok(newest_memory_page(policies, page, |stored| {
            (&stored.policy.created_at, &stored.policy.id.0)
        }))
    }

    async fn policy_usage(
        &self,
        id: &SponsorshipPolicyId,
    ) -> Result<SponsorshipPolicyUsage, SponsorshipStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .policy_usage
            .get(&id.0)
            .cloned()
            .unwrap_or(SponsorshipPolicyUsage {
                consumed: SponsoredUsageSummary::default(),
                reserved: SponsoredUsageSummary::default(),
                active_launches: 0,
            }))
    }

    async fn replace_policy(
        &self,
        expected_generation: u64,
        policy: &SponsorshipPolicy,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipPolicy, SponsorshipStoreError> {
        self.policy_replace_attempts.fetch_add(1, Ordering::SeqCst);
        if self
            .policy_replace_stale_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        let mut state = self.state.lock().await;
        let stored = state
            .policies
            .get_mut(&policy.id.0)
            .ok_or(SponsorshipStoreError::Conflict)?;
        if stored.state == SponsorshipPolicyState::Revoked {
            return Ok(stored.clone());
        }
        if stored.generation != expected_generation {
            if stored == policy {
                return Ok(stored.clone());
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        *stored = policy.clone();
        state.audits.push(audit);
        Ok(policy.clone())
    }

    async fn insert_acceptance(
        &self,
        command: PersistSponsoredAcceptance,
    ) -> Result<SponsoredGrantAcceptance, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        command
            .acceptance
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if command.acceptance.idempotency_key_hash
            != command.idempotency_candidates.current().as_str()
        {
            return Err(SponsorshipStoreError::Malformed(
                "new acceptance canonical idempotency identity is not current".into(),
            ));
        }
        let launch_id = command.acceptance.launch_id.0.clone();
        if let Some(stored) = state.acceptances.get(&launch_id).cloned() {
            if stored.contributor != command.acceptance.contributor {
                return Err(SponsorshipStoreError::LaunchAuthorityChanged);
            }
            if acceptance_identity_matches(&stored, &command.idempotency_candidates) {
                if acceptance_payload_fields_match(&stored, &command.acceptance) {
                    state.acceptance_aliases.insert(
                        launch_id,
                        pseudonym_aliases(&command.idempotency_candidates),
                    );
                    return Ok(stored);
                }
                return Err(SponsorshipStoreError::Conflict);
            }
            return Err(SponsorshipStoreError::Conflict);
        }
        require_gate(&state, &command.gate_fence)?;
        let current_launch = state
            .launches
            .get(&command.launch_authority.launch_id.0)
            .ok_or(SponsorshipStoreError::LaunchAuthorityChanged)?;
        if current_launch != &command.launch_authority
            || current_launch.state != SponsoredLaunchState::TermsReview
            || current_launch.session_id.is_some()
            || command.acceptance.launch_id != current_launch.launch_id
            || command.acceptance.contributor != current_launch.contributor
            || command.acceptance.revision_id != current_launch.revision_id
            || command.acceptance.revision_digest != current_launch.revision_digest
            || command.acceptance.policy_id != current_launch.policy_id
            || command.acceptance.launch_snapshot_digest != current_launch.launch_snapshot_digest
        {
            return Err(SponsorshipStoreError::LaunchAuthorityChanged);
        }
        state.acceptances.insert(
            command.acceptance.launch_id.0.clone(),
            command.acceptance.clone(),
        );
        state.acceptance_aliases.insert(
            command.acceptance.launch_id.0.clone(),
            pseudonym_aliases(&command.idempotency_candidates),
        );
        state.audits.push(command.audit);
        Ok(command.acceptance)
    }

    async fn acceptance_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrantAcceptance>, SponsorshipStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .acceptances
            .get(&launch_id.0)
            .cloned())
    }

    async fn acceptance_for_request(
        &self,
        launch_id: &LaunchId,
        idempotency_candidates: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<SponsoredGrantAcceptance>, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let Some(acceptance) = state.acceptances.get(&launch_id.0).cloned() else {
            return Ok(None);
        };
        if acceptance_identity_matches(&acceptance, idempotency_candidates) {
            state.acceptance_aliases.insert(
                launch_id.0.clone(),
                pseudonym_aliases(idempotency_candidates),
            );
        }
        Ok(Some(acceptance))
    }

    async fn claim_grant_activation(
        &self,
        command: ClaimGrantActivation,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let authoritative_now = state.authoritative_now.clone();
        if command.candidate.lease.is_some() {
            return Err(SponsorshipStoreError::Malformed(
                "activation claim candidates cannot supply a lease; persistence must use database time"
                    .into(),
            ));
        }
        command
            .candidate
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if command.candidate.billing_request_id
            != command.billing_request_candidates.current().as_str()
            || command.candidate.provider_request_id
                != command.provider_request_candidates.current().as_str()
        {
            return Err(SponsorshipStoreError::Malformed(
                "new activation external operation identities are not current".into(),
            ));
        }
        require_activation_dependencies(
            &state,
            &command.candidate,
            &command.launch_authority,
            &command.acceptance,
            &authoritative_now,
        )?;
        let candidate = command.candidate;
        if let Some(existing) = state.activations.get(&candidate.launch_id.0).cloned() {
            if existing.request_fingerprint != candidate.request_fingerprint
                || !command
                    .billing_request_candidates
                    .iter()
                    .any(|identity| identity.as_str() == existing.billing_request_id)
                || !command
                    .provider_request_candidates
                    .iter()
                    .any(|identity| identity.as_str() == existing.provider_request_id)
            {
                return Err(SponsorshipStoreError::Conflict);
            }
            if matches!(
                existing.state,
                GrantActivationState::Committed | GrantActivationState::Denied
            ) {
                return Ok(GrantActivationClaim {
                    acquired: false,
                    intent: existing,
                });
            }
            let held = existing
                .lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > authoritative_now);
            if held {
                return Ok(GrantActivationClaim {
                    acquired: false,
                    intent: existing,
                });
            }
            let mut claimed = existing;
            if !claimed.capacity_held {
                require_current_terms(&state, &claimed, &command.acceptance)?;
                reserve_activation_allocation(&mut state, &claimed)?;
                claimed.capacity_held = true;
            }
            claimed.lease = Some(test_activation_lease(
                command.holder_candidates.current().as_str().to_string(),
                &authoritative_now,
            )?);
            claimed.revision += 1;
            claimed.updated_at = authoritative_now;
            claimed
                .validate()
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
            state
                .activations
                .insert(candidate.launch_id.0.clone(), claimed.clone());
            return Ok(GrantActivationClaim {
                acquired: true,
                intent: claimed,
            });
        }
        let mut candidate = candidate;
        require_current_terms(&state, &candidate, &command.acceptance)?;
        reserve_activation_allocation(&mut state, &candidate)?;
        candidate.capacity_held = true;
        candidate.lease = Some(test_activation_lease(
            command.holder_candidates.current().as_str().to_string(),
            &authoritative_now,
        )?);
        candidate.updated_at = authoritative_now;
        candidate
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        state
            .activations
            .insert(candidate.launch_id.0.clone(), candidate.clone());
        Ok(GrantActivationClaim {
            acquired: true,
            intent: candidate.clone(),
        })
    }

    async fn grant_activation(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<GrantActivationIntent>, SponsorshipStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .activations
            .get(&launch_id.0)
            .cloned())
    }

    async fn claim_grant_activation_cleanup(
        &self,
        command: ClaimGrantActivationCleanup,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        command
            .operation
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if command.operation.kind != ProviderOperationKind::RevokeUncommittedGrant
            || command.operation.phase != ProviderOperationPhase::Prepared
            || command.operation.client_request_id_digest
                != command
                    .operation_client_request_candidates
                    .current()
                    .as_str()
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        let authoritative_now = state.authoritative_now.clone();
        let mut intent = state
            .activations
            .get(&command.launch_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if intent.state == GrantActivationState::Cleaned {
            return Ok(GrantActivationClaim {
                acquired: false,
                intent,
            });
        }
        if !intent.capacity_held || intent.provider_receipt.is_none() {
            return Err(SponsorshipStoreError::Conflict);
        }
        if activation_dependencies_are_current(&state, &intent, &authoritative_now) {
            return Err(SponsorshipStoreError::Conflict);
        }
        let held = intent
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at > authoritative_now);
        match intent.state {
            GrantActivationState::ProviderConfirmed | GrantActivationState::ProviderAmbiguous => {
                if held
                    && !intent.lease.as_ref().is_some_and(|lease| {
                        command
                            .holder_candidates
                            .iter()
                            .any(|candidate| candidate.as_str() == lease.holder)
                    })
                {
                    return Ok(GrantActivationClaim {
                        acquired: false,
                        intent,
                    });
                }
                intent.cleanup_operation = Some(command.operation);
                intent.state = GrantActivationState::CleanupPending;
            }
            GrantActivationState::CleanupPending => {
                let existing = intent
                    .cleanup_operation
                    .as_ref()
                    .ok_or(SponsorshipStoreError::Conflict)?;
                if !command
                    .operation_client_request_candidates
                    .iter()
                    .any(|candidate| candidate.as_str() == existing.client_request_id_digest)
                {
                    return Err(SponsorshipStoreError::Conflict);
                }
                if held {
                    return Ok(GrantActivationClaim {
                        acquired: false,
                        intent,
                    });
                }
            }
            _ => return Err(SponsorshipStoreError::Conflict),
        }
        intent.revision += 1;
        intent.lease = Some(test_activation_lease(
            command.holder_candidates.current().as_str().to_string(),
            &authoritative_now,
        )?);
        intent.updated_at = authoritative_now;
        intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        state
            .activations
            .insert(command.launch_id.0, intent.clone());
        Ok(GrantActivationClaim {
            acquired: true,
            intent,
        })
    }

    async fn claim_invalidated_grant_activation(
        &self,
        command: ClaimInvalidatedGrantActivation,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let authoritative_now = state.authoritative_now.clone();
        let mut intent = state
            .activations
            .get(&command.launch_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if activation_dependencies_are_current(&state, &intent, &authoritative_now)
            || !intent.capacity_held
            || !matches!(
                intent.state,
                GrantActivationState::BillingAttempting
                    | GrantActivationState::BillingAmbiguous
                    | GrantActivationState::ProviderAttempting
                    | GrantActivationState::ProviderAmbiguous
            )
            || (intent.state == GrantActivationState::ProviderAmbiguous
                && intent.provider_receipt.is_some())
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        if intent
            .lease
            .as_ref()
            .is_some_and(|lease| lease.expires_at > authoritative_now)
        {
            return Ok(GrantActivationClaim {
                acquired: false,
                intent,
            });
        }
        intent.revision += 1;
        intent.lease = Some(test_activation_lease(
            command.holder_candidates.current().as_str().to_string(),
            &authoritative_now,
        )?);
        intent.updated_at = authoritative_now;
        intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        state
            .activations
            .insert(command.launch_id.0, intent.clone());
        Ok(GrantActivationClaim {
            acquired: true,
            intent,
        })
    }

    async fn begin_provider_grant_issue(
        &self,
        command: BeginProviderGrantIssue,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let authoritative_now = state.authoritative_now.clone();
        command
            .launch_authority
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .acceptance
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let mut intent = state
            .activations
            .get(&command.launch_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if intent.revision != command.expected_revision
            || intent.lease.as_ref().map(|lease| lease.holder.as_str())
                != Some(command.holder.as_str())
            || !matches!(
                intent.state,
                GrantActivationState::BillingAttempting
                    | GrantActivationState::BillingAmbiguous
                    | GrantActivationState::AwaitingProvider
            )
            || !intent.capacity_held
            || intent.provider_receipt.is_some()
            || intent.cleanup_operation.is_some()
        {
            return Err(SponsorshipStoreError::StaleGeneration);
        }

        intent.revision += 1;
        intent.updated_at = authoritative_now.clone();
        if require_activation_dependencies(
            &state,
            &intent,
            &command.launch_authority,
            &command.acceptance,
            &authoritative_now,
        )
        .is_err()
        {
            intent.state = GrantActivationState::Denied;
            intent.denial = Some(GrantActivationDenial::AuthorityInvalidated);
            intent.capacity_held = false;
            intent.lease = None;
            release_activation_allocation(&mut state, &intent)?;
        } else {
            intent.state = GrantActivationState::ProviderAttempting;
            intent.denial = None;
        }
        intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        state
            .activations
            .insert(command.launch_id.0, intent.clone());
        Ok(intent)
    }

    async fn update_grant_activation(
        &self,
        expected_revision: u64,
        holder: &str,
        intent: &GrantActivationIntent,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let stored = state
            .activations
            .get(&intent.launch_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if stored.revision != expected_revision
            || stored.lease.as_ref().map(|lease| lease.holder.as_str()) != Some(holder)
        {
            if &stored == intent {
                return Ok(stored);
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if stored.capacity_held && !intent.capacity_held {
            release_activation_allocation(&mut state, &stored)?;
        } else if !stored.capacity_held && intent.capacity_held {
            return Err(SponsorshipStoreError::Conflict);
        }
        state
            .activations
            .insert(intent.launch_id.0.clone(), intent.clone());
        Ok(intent.clone())
    }

    async fn commit_grant_activation(
        &self,
        command: CommitGrantActivation,
    ) -> Result<SponsoredGrant, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let authoritative_now = state.authoritative_now.clone();
        let stored = state
            .activations
            .get(&command.intent.launch_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if stored.revision != command.expected_revision
            || stored.lease.as_ref().map(|lease| lease.holder.as_str())
                != Some(command.holder.as_str())
            || stored.state != GrantActivationState::ProviderConfirmed
            || !stored.capacity_held
            || command.intent.state != GrantActivationState::Committed
            || command.intent.capacity_held
            || stored.provider_receipt.is_none()
            || command.intent.provider_receipt != stored.provider_receipt
        {
            if let Some(id) = state.grants_by_launch.get(&command.grant.launch_id.0) {
                return state
                    .grants
                    .get(id)
                    .cloned()
                    .ok_or(SponsorshipStoreError::Conflict);
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        require_activation_dependencies(
            &state,
            &stored,
            &command.launch_authority,
            &command.acceptance,
            &authoritative_now,
        )?;
        require_gate(&state, &command.gate_fence)?;
        command
            .grant
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if command.intent.launch_authority_digest != command.launch_authority.digest
            || command.intent.acceptance_digest != command.acceptance.digest
            || command.grant.acceptance != command.acceptance
            || command.grant.allocation != stored.allocation
            || command.grant.policy_id != stored.policy_id
            || command.grant.policy_generation != stored.policy_generation
            || command.grant.credential_generation != stored.credential_generation
            || command.grant.launch_id != command.launch_authority.launch_id
            || command.grant.contributor != command.launch_authority.contributor
            || command.grant.trigger_revision_id != command.launch_authority.revision_id
            || command.grant.trigger_revision_digest != command.launch_authority.revision_digest
            || command.launch_authority.session_id.as_ref() != Some(&command.grant.session_id)
        {
            return Err(SponsorshipStoreError::LaunchAuthorityChanged);
        }
        let receipt = stored.provider_receipt.clone().unwrap();
        let database_now = crate::seconds_from_rfc3339(&authoritative_now)
            .ok_or_else(|| SponsorshipStoreError::Malformed("invalid database clock".into()))?;
        let receipt_issued = crate::seconds_from_rfc3339(&receipt.issued_at)
            .ok_or_else(|| SponsorshipStoreError::Malformed("invalid receipt issue time".into()))?;
        let receipt_expiry = crate::seconds_from_rfc3339(&receipt.expires_at)
            .ok_or_else(|| SponsorshipStoreError::Malformed("invalid receipt expiry".into()))?;
        if receipt_issued > database_now || receipt_expiry <= database_now {
            return Err(SponsorshipStoreError::GrantUnavailable);
        }
        require_receipt_matches_grant(&receipt, &command.grant)
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if state
            .grants_by_launch
            .contains_key(&command.grant.launch_id.0)
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        state.grants_by_launch.insert(
            command.grant.launch_id.0.clone(),
            command.grant.id.0.clone(),
        );
        state
            .grants
            .insert(command.grant.id.0.clone(), command.grant.clone());
        state
            .grant_receipts
            .insert(command.grant.id.0.clone(), receipt);
        state
            .activations
            .insert(command.intent.launch_id.0.clone(), command.intent.clone());
        state.audits.push(command.audit);
        Ok(command.grant)
    }

    async fn complete_grant_activation_cleanup(
        &self,
        command: CompleteGrantActivationCleanup,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let stored = state
            .activations
            .get(&command.intent.launch_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if stored == command.intent {
            return Ok(stored);
        }
        let stored_operation = stored
            .cleanup_operation
            .as_ref()
            .ok_or(SponsorshipStoreError::StaleGeneration)?;
        let global_credential_retirement = if command.global_credential_retirement {
            let policy = state
                .policies
                .get(&stored.policy_id.0)
                .ok_or(SponsorshipStoreError::StaleGeneration)?;
            let credential = state
                .credentials
                .get(&policy.credential_id.0)
                .map(|(record, _)| record)
                .ok_or(SponsorshipStoreError::StaleGeneration)?;
            credential_retirement_covers_activation(&stored, policy, credential)
        } else {
            false
        };
        let cleanup_completion_is_authorized = matches!(
            stored_operation.phase,
            ProviderOperationPhase::Attempting | ProviderOperationPhase::Ambiguous
        ) || (stored_operation.phase
            == ProviderOperationPhase::Rejected
            && global_credential_retirement);
        if stored.revision != command.expected_revision
            || stored.lease.as_ref().map(|lease| lease.holder.as_str())
                != Some(command.holder.as_str())
            || stored.state != GrantActivationState::CleanupPending
            || !stored.capacity_held
            || stored.provider_receipt != command.intent.provider_receipt
            || command.intent.state != GrantActivationState::Cleaned
            || command.intent.capacity_held
            || command.intent.lease.is_some()
            || command.intent.cleanup_operation.is_some()
            || (command.global_credential_retirement && !global_credential_retirement)
            || !cleanup_completion_is_authorized
            || stored_operation.kind != command.operation.kind
            || stored_operation.provider_request_id != command.operation.provider_request_id
            || stored_operation.client_request_id_digest
                != command.operation.client_request_id_digest
            || stored_operation.payload_digest != command.operation.payload_digest
            || stored_operation.requested_at != command.operation.requested_at
            || command.intent.updated_at < stored.updated_at
            || command.audit.occurred_at != command.intent.updated_at
            || command.expected_revision.checked_add(1) != Some(command.intent.revision)
            || stored.launch_id != command.intent.launch_id
            || stored.session_id != command.intent.session_id
            || stored.policy_id != command.intent.policy_id
            || stored.policy_generation != command.intent.policy_generation
            || stored.credential_generation != command.intent.credential_generation
            || stored.launch_authority_digest != command.intent.launch_authority_digest
            || stored.acceptance_digest != command.intent.acceptance_digest
            || stored.gate_fence != command.intent.gate_fence
            || stored.allocation != command.intent.allocation
            || stored.request_fingerprint != command.intent.request_fingerprint
            || stored.billing_request_id != command.intent.billing_request_id
            || stored.provider_request_id != command.intent.provider_request_id
            || stored.created_at != command.intent.created_at
        {
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        command
            .intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        release_activation_allocation(&mut state, &stored)?;
        state
            .activations
            .insert(command.intent.launch_id.0.clone(), command.intent.clone());
        state.audits.push(command.audit);
        Ok(command.intent)
    }

    async fn complete_invalidated_grant_activation(
        &self,
        command: CompleteInvalidatedGrantActivation,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let stored = state
            .activations
            .get(&command.intent.launch_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if stored == command.intent {
            return Ok(stored);
        }
        if stored.revision != command.expected_revision
            || stored.lease.as_ref().map(|lease| lease.holder.as_str())
                != Some(command.holder.as_str())
            || !stored.capacity_held
            || command.intent.state != GrantActivationState::Denied
            || command.intent.denial != Some(GrantActivationDenial::AuthorityInvalidated)
            || command.intent.capacity_held
            || command.intent.lease.is_some()
        {
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        command
            .intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        release_activation_allocation(&mut state, &stored)?;
        state
            .activations
            .insert(command.intent.launch_id.0.clone(), command.intent.clone());
        state.audits.push(command.audit);
        Ok(command.intent)
    }

    async fn provider_grant_receipt(
        &self,
        grant_id: &SponsoredGrantId,
    ) -> Result<Option<ProviderGrantReceipt>, SponsorshipStoreError> {
        self.provider_receipt_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .state
            .lock()
            .await
            .grant_receipts
            .get(&grant_id.0)
            .cloned())
    }

    async fn grant(
        &self,
        id: &SponsoredGrantId,
    ) -> Result<Option<SponsoredGrant>, SponsorshipStoreError> {
        Ok(self.state.lock().await.grants.get(&id.0).cloned())
    }

    async fn grants_for_contributor(
        &self,
        contributor: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredGrant>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        Ok(newest_memory_page(
            state
                .grants
                .values()
                .filter(|grant| &grant.contributor == contributor)
                .cloned()
                .collect(),
            page,
            |grant| (&grant.created_at, &grant.id.0),
        ))
    }

    async fn grants_for_sponsor(
        &self,
        sponsor: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredGrant>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        Ok(newest_memory_page(
            state
                .grants
                .values()
                .filter(|grant| {
                    state
                        .policies
                        .get(&grant.policy_id.0)
                        .is_some_and(|policy| policy.owner.controller() == sponsor)
                })
                .cloned()
                .collect(),
            page,
            |grant| (&grant.created_at, &grant.id.0),
        ))
    }

    async fn grant_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrant>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        Ok(state
            .grants_by_launch
            .get(&launch_id.0)
            .and_then(|id| state.grants.get(id))
            .cloned())
    }

    async fn usage_reservation(
        &self,
        reservation_id: &SponsoredUsageReservationId,
    ) -> Result<Option<SponsoredUsageReservation>, SponsorshipStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .reservations
            .get(&reservation_id.0)
            .cloned())
    }

    async fn usage_reservation_for_request(
        &self,
        request_identity: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<SponsoredUsageReservation>, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let candidates = pseudonym_aliases(request_identity);
        let matches = state
            .reservation_aliases
            .iter()
            .filter_map(|(reservation_id, aliases)| {
                (!aliases.is_disjoint(&candidates)).then_some(reservation_id.clone())
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(SponsorshipStoreError::Malformed(
                "usage request candidates resolve multiple reservations".into(),
            ));
        }
        let Some(reservation_id) = matches.into_iter().next() else {
            return Ok(None);
        };
        let reservation = state
            .reservations
            .get(&reservation_id)
            .cloned()
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "usage request alias references a missing reservation".into(),
                )
            })?;
        state.reservation_aliases.insert(reservation_id, candidates);
        Ok(Some(reservation))
    }

    async fn replace_grant(
        &self,
        expected_generation: u64,
        grant: &SponsoredGrant,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredGrant, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let stored = state
            .grants
            .get(&grant.id.0)
            .ok_or(SponsorshipStoreError::Conflict)?;
        if stored.generation != expected_generation {
            if stored == grant {
                return Ok(stored.clone());
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        grant
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if stored.usage != grant.usage || stored.allocation != grant.allocation {
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        state.grants.insert(grant.id.0.clone(), grant.clone());
        state.audits.push(audit);
        Ok(grant.clone())
    }

    async fn release_grant_capacity(
        &self,
        expected_generation: u64,
        grant: &SponsoredGrant,
        reason: GrantCapacityRelease,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredGrant, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let stored = state
            .grants
            .get(&grant.id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if stored.generation != expected_generation {
            if &stored == grant {
                return Ok(stored);
            }
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        let expected_state = match reason {
            GrantCapacityRelease::Expired => SponsoredGrantState::Expired,
            GrantCapacityRelease::Exhausted => SponsoredGrantState::Exhausted,
            GrantCapacityRelease::Revoked => SponsoredGrantState::Revoked,
            GrantCapacityRelease::Closed => SponsoredGrantState::Closed,
        };
        if grant.state != expected_state
            || stored.policy_id != grant.policy_id
            || stored.usage != grant.usage
            || stored.allocation != grant.allocation
            || grant.updated_at < stored.updated_at
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        if reason == GrantCapacityRelease::Expired {
            let database_now = crate::seconds_from_rfc3339(&state.authoritative_now)
                .ok_or_else(|| SponsorshipStoreError::Malformed("invalid database clock".into()))?;
            let expiry =
                crate::seconds_from_rfc3339(&stored.authority.expires_at).ok_or_else(|| {
                    SponsorshipStoreError::Malformed("grant expiry is malformed".into())
                })?;
            if database_now < expiry {
                return Err(SponsorshipStoreError::GrantUnavailable);
            }
        }
        release_terminal_grant_capacity(&mut state, &stored, None)?;
        state.grants.insert(grant.id.0.clone(), grant.clone());
        state.audits.push(audit);
        Ok(grant.clone())
    }

    async fn reserve_usage(
        &self,
        command: ReserveSponsoredUsage,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredUsageReservationOutcome, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let authoritative_now = state.authoritative_now.clone();
        let fingerprint = command
            .request
            .fingerprint()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let request_aliases = pseudonym_aliases(&command.request.request_identity);
        let matches = state
            .reservation_aliases
            .iter()
            .filter_map(|(reservation_id, aliases)| {
                (!aliases.is_disjoint(&request_aliases)).then_some(reservation_id.clone())
            })
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(SponsorshipStoreError::Malformed(
                "usage request candidates resolve multiple reservations".into(),
            ));
        }
        if let Some(reservation_id) = matches.into_iter().next() {
            let existing = state
                .reservations
                .get(&reservation_id)
                .cloned()
                .ok_or_else(|| {
                    SponsorshipStoreError::Malformed(
                        "usage request alias references a missing reservation".into(),
                    )
                })?;
            if existing.idempotency_fingerprint != command.request.idempotency_fingerprint {
                return Err(SponsorshipStoreError::Conflict);
            }
            state
                .reservation_aliases
                .insert(reservation_id, request_aliases);
            return Ok(SponsoredUsageReservationOutcome {
                disposition: ReservationDisposition::Existing,
                reservation: existing,
            });
        }
        require_gate(&state, &command.gate_fence)?;
        command
            .price_quote
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if command.request.price_quote_digest != command.price_quote.digest
            || command.request.maximum != command.price_quote.maximum
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        let current_launch = state
            .launches
            .get(&command.launch_authority.launch_id.0)
            .ok_or(SponsorshipStoreError::LaunchAuthorityChanged)?;
        if current_launch != &command.launch_authority
            || !matches!(
                current_launch.state,
                SponsoredLaunchState::Active | SponsoredLaunchState::Recoverable
            )
        {
            return Err(SponsorshipStoreError::LaunchAuthorityChanged);
        }
        let grant = state
            .grants
            .get(&command.request.grant_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::GrantUnavailable)?;
        let policy = state
            .policies
            .get(&grant.policy_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::GrantUnavailable)?;
        let credential = state
            .credentials
            .get(&policy.credential_id.0)
            .map(|(record, _)| record.clone())
            .ok_or(SponsorshipStoreError::GrantUnavailable)?;
        if !grant.is_active_at(&authoritative_now)
            || !policy.is_available_at(&authoritative_now)
            || credential.state != SponsoredCredentialState::Active
            || grant.policy_generation != command.policy_generation
            || policy.generation != command.policy_generation
            || grant.credential_generation != command.credential_generation
            || grant.contributor != command.request.requester
            || grant.capability.fingerprint != command.request.capability_fingerprint
            || grant.tenant_id != command.request.tenant_id
            || grant.trigger_revision_id != command.request.trigger_revision_id
            || grant.trigger_revision_digest != command.request.trigger_revision_digest
            || grant.launch_id != command.request.launch_id
            || grant.session_id != command.request.session_id
            || grant.launch_id != current_launch.launch_id
            || current_launch.session_id.as_ref() != Some(&grant.session_id)
            || current_launch.contributor != grant.contributor
            || command.request.provider != policy.provider_policy.provider
            || !policy
                .provider_policy
                .allowed_models
                .contains(&command.request.model)
            || !policy
                .provider_policy
                .allowed_resources
                .contains(&command.request.resource)
        {
            return Err(SponsorshipStoreError::GrantUnavailable);
        }

        let mut launch_held = SponsoredUsageSummary::default();
        for reservation in state.reservations.values().filter(|reservation| {
            matches!(
                reservation.state,
                SponsoredUsageState::Reserved | SponsoredUsageState::Ambiguous
            )
        }) {
            if reservation.grant_id == grant.id {
                launch_held = add_usage(launch_held, &reservation.reserved)?;
            }
        }
        let launch_projected = add_usage(
            add_usage(grant.usage.clone(), &launch_held)?,
            &command.request.maximum,
        )?;
        if !launch_projected.quota.fits_within(grant.allocation.quota) {
            return Err(SponsorshipStoreError::LaunchQuotaExceeded);
        }
        if launch_projected.spend_micros > grant.allocation.spend_micros {
            return Err(SponsorshipStoreError::LaunchBudgetExceeded);
        }
        let now = crate::seconds_from_rfc3339(&authoritative_now)
            .ok_or_else(|| SponsorshipStoreError::Malformed("invalid database clock".into()))?;
        let recent = state
            .reservation_times
            .entry(policy.id.0.clone())
            .or_default();
        recent.retain(|observed| observed.saturating_add(60) > now);
        if recent.len() >= policy.max_requests_per_minute as usize {
            return Err(SponsorshipStoreError::RateExceeded);
        }
        recent.push(now);

        let reservation = SponsoredUsageReservation {
            id: command.request.reservation_id,
            idempotency_fingerprint: command.request.idempotency_fingerprint,
            request_fingerprint: fingerprint,
            grant_id: grant.id,
            price_quote: command.price_quote,
            reserved: command.request.maximum,
            state: SponsoredUsageState::Reserved,
            created_at: command.now.clone(),
            updated_at: command.now,
            actual: None,
        };
        reservation
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        state
            .reservation_policy
            .insert(reservation.id.0.clone(), policy.id.0.clone());
        state
            .reservation_aliases
            .insert(reservation.id.0.clone(), request_aliases);
        state
            .reservations
            .insert(reservation.id.0.clone(), reservation.clone());
        state.audits.push(audit);
        Ok(SponsoredUsageReservationOutcome {
            disposition: ReservationDisposition::Created,
            reservation,
        })
    }

    async fn reconcile_usage(
        &self,
        command: ReconcileSponsoredUsage,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredUsageReservation, SponsorshipStoreError> {
        let mut state = self.state.lock().await;
        let current = state
            .reservations
            .get(&command.reservation_id.0)
            .cloned()
            .ok_or(SponsorshipStoreError::Conflict)?;
        if current.request_fingerprint != command.request_fingerprint {
            return Err(SponsorshipStoreError::Conflict);
        }
        if current.state == command.outcome
            && current.actual == command.actual
            && command.actual_quota == current.actual.as_ref().map(|actual| actual.quota)
        {
            return Ok(current);
        }
        if matches!(
            current.state,
            SponsoredUsageState::Settled | SponsoredUsageState::Released
        ) || !matches!(
            command.outcome,
            SponsoredUsageState::Settled
                | SponsoredUsageState::Released
                | SponsoredUsageState::Ambiguous
        ) || command.observed_at < current.updated_at
            || audit.occurred_at != command.observed_at
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        if let Some(actual) = command.actual.as_ref() {
            if command.actual_quota != Some(actual.quota)
                || !actual.quota.fits_within(current.reserved.quota)
                || actual.spend_micros > current.reserved.spend_micros
            {
                return Err(SponsorshipStoreError::Conflict);
            }
        }
        let mut replacement = current.clone();
        replacement.state = command.outcome;
        replacement.actual = command.actual.clone();
        replacement.updated_at = command.observed_at;
        replacement
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if let Some(actual) = command
            .actual
            .filter(|_| command.outcome == SponsoredUsageState::Settled)
        {
            let mut grant = state
                .grants
                .remove(&current.grant_id.0)
                .ok_or(SponsorshipStoreError::Conflict)?;
            grant.usage = add_usage(grant.usage.clone(), &actual)?;
            if grant.updated_at < replacement.updated_at {
                grant.updated_at = replacement.updated_at.clone();
            }
            let capacity_was_released = grant_capacity_is_released(grant.state);
            let becomes_exhausted = !capacity_was_released
                && matches!(
                    grant.state,
                    SponsoredGrantState::Active | SponsoredGrantState::Suspended
                )
                && grant.usage.reaches_any_limit(&grant.allocation);
            let policy_id = grant.policy_id.0.clone();
            {
                let usage = state
                    .policy_usage
                    .get_mut(&policy_id)
                    .ok_or(SponsorshipStoreError::Conflict)?;
                usage.consumed = add_usage(usage.consumed.clone(), &actual)?;
                let released_hold = if capacity_was_released {
                    &current.reserved
                } else {
                    &actual
                };
                usage.reserved = usage
                    .reserved
                    .checked_sub(released_hold)
                    .ok_or(SponsorshipStoreError::Conflict)?;
            }
            if becomes_exhausted {
                grant.generation = grant.generation.checked_add(1).ok_or_else(|| {
                    SponsorshipStoreError::Malformed("grant generation exhausted".into())
                })?;
                grant.state = SponsoredGrantState::Exhausted;
                grant.updated_at = replacement.updated_at.clone();
                grant
                    .refresh_capability()
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                release_terminal_grant_capacity(&mut state, &grant, Some(&current.id.0))?;
            }
            grant
                .validate()
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
            state.grants.insert(grant.id.0.clone(), grant);
        } else if command.outcome == SponsoredUsageState::Released {
            let released_policy_id = {
                let grant = state
                    .grants
                    .get(&current.grant_id.0)
                    .ok_or(SponsorshipStoreError::Conflict)?;
                grant_capacity_is_released(grant.state).then(|| grant.policy_id.0.clone())
            };
            if let Some(policy_id) = released_policy_id {
                let usage = state
                    .policy_usage
                    .get_mut(&policy_id)
                    .ok_or(SponsorshipStoreError::Conflict)?;
                usage.reserved = usage
                    .reserved
                    .checked_sub(&current.reserved)
                    .ok_or(SponsorshipStoreError::Conflict)?;
            }
        }
        state
            .reservations
            .insert(command.reservation_id.0, replacement.clone());
        state.audits.push(audit);
        Ok(replacement)
    }

    async fn audit_events(
        &self,
        actor_digest: &str,
        limit: usize,
    ) -> Result<Vec<SponsoredAuditEvent>, SponsorshipStoreError> {
        Ok(self
            .state
            .lock()
            .await
            .audits
            .iter()
            .filter(|event| event.actor_digest == actor_digest)
            .rev()
            .take(limit.min(MAX_SPONSORED_AUDIT_PAGE))
            .cloned()
            .collect())
    }

    async fn credential_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredCredentialId>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        let rows = state
            .credentials
            .values()
            .map(|(record, _)| record)
            .filter(|record| {
                matches!(
                    record.state,
                    SponsoredCredentialState::RotationPending
                        | SponsoredCredentialState::RevocationPending
                )
            })
            .map(|record| (record.updated_at.clone(), record.id.clone()))
            .collect::<Vec<_>>();
        recovery_memory_page(rows, page, "credential_recovery", |row| (&row.0, &row.1 .0)).map(
            |page| SponsorshipRecoveryPage {
                items: page.items.into_iter().map(|(_, id)| id).collect(),
                next: page.next,
            },
        )
    }

    async fn activation_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<LaunchId>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        let rows = state
            .activations
            .values()
            .filter(|intent| {
                matches!(
                    intent.state,
                    GrantActivationState::BillingAttempting
                        | GrantActivationState::BillingAmbiguous
                        | GrantActivationState::ProviderAttempting
                        | GrantActivationState::ProviderAmbiguous
                        | GrantActivationState::ProviderConfirmed
                        | GrantActivationState::CleanupPending
                )
            })
            .map(|intent| (intent.updated_at.clone(), intent.launch_id.clone()))
            .collect::<Vec<_>>();
        recovery_memory_page(rows, page, "activation_recovery", |row| (&row.0, &row.1 .0)).map(
            |page| SponsorshipRecoveryPage {
                items: page.items.into_iter().map(|(_, id)| id).collect(),
                next: page.next,
            },
        )
    }

    async fn grant_recovery_candidates(
        &self,
        as_of: &str,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredGrantId>, SponsorshipStoreError> {
        if crate::seconds_from_rfc3339(as_of).is_none() {
            return Err(SponsorshipStoreError::Malformed(
                "grant recovery clock fence is not a CMA timestamp".into(),
            ));
        }
        let state = self.state.lock().await;
        let rows = state
            .grants
            .values()
            .filter_map(|grant| match grant.state {
                SponsoredGrantState::RevocationPending if grant.updated_at.as_str() <= as_of => {
                    Some((grant.updated_at.clone(), grant.id.clone()))
                }
                SponsoredGrantState::Active | SponsoredGrantState::Suspended
                    if grant.authority.expires_at.as_str() <= as_of =>
                {
                    Some((grant.authority.expires_at.clone(), grant.id.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let domain = format!("grant_recovery:{as_of}");
        recovery_memory_page(rows, page, &domain, |row| (&row.0, &row.1 .0)).map(|page| {
            SponsorshipRecoveryPage {
                items: page.items.into_iter().map(|(_, id)| id).collect(),
                next: page.next,
            }
        })
    }

    async fn usage_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredUsageReservationId>, SponsorshipStoreError> {
        let state = self.state.lock().await;
        let rows = state
            .reservations
            .values()
            .filter(|reservation| reservation.state == SponsoredUsageState::Ambiguous)
            .map(|reservation| (reservation.updated_at.clone(), reservation.id.clone()))
            .collect::<Vec<_>>();
        recovery_memory_page(rows, page, "usage_recovery", |row| (&row.0, &row.1 .0)).map(|page| {
            SponsorshipRecoveryPage {
                items: page.items.into_iter().map(|(_, id)| id).collect(),
                next: page.next,
            }
        })
    }

    async fn ciphertext_rewrap_candidates(
        &self,
        _kind: SponsorshipCiphertextRecordKind,
        _retiring_key_id: &str,
        _page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsorshipCiphertextRewrapCandidate>, SponsorshipStoreError>
    {
        Ok(SponsorshipRecoveryPage {
            items: Vec::new(),
            next: None,
        })
    }

    async fn rewrap_ciphertext(
        &self,
        _candidate: &SponsorshipCiphertextRewrapCandidate,
    ) -> Result<bool, SponsorshipStoreError> {
        Ok(false)
    }
}

fn credential_registration_bindings_match(
    stored: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    stored.owner == candidate.owner
        && stored.authority.provider == candidate.authority.provider
        && stored.authority.provider_policy_key == candidate.authority.provider_policy_key
        && stored.authority.supports_expiry == candidate.authority.supports_expiry
        && stored.authority.supports_revocation == candidate.authority.supports_revocation
}

fn credential_operation_completion(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
    kind: ProviderOperationKind,
    outcome: CompletedProviderOperationOutcome,
) -> Result<CompletedProviderOperation, SponsorshipStoreError> {
    let pending = current
        .pending_operation
        .as_ref()
        .ok_or(SponsorshipStoreError::Conflict)?;
    let completed = candidate
        .last_completed_operation
        .as_ref()
        .ok_or(SponsorshipStoreError::Conflict)?;
    if pending.kind != kind
        || completed.kind != kind
        || completed.outcome != outcome
        || pending.provider_request_id != completed.provider_request_id
        || pending.client_request_id_digest != completed.client_request_id_digest
        || pending.payload_digest != completed.payload_digest
    {
        return Err(SponsorshipStoreError::Conflict);
    }
    if current.id != candidate.id {
        return Err(SponsorshipStoreError::Conflict);
    }
    completed
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(completed.clone())
}

fn credential_operation_key(
    record: &SponsoredCredentialRecord,
    completed: &CompletedProviderOperation,
) -> (String, ProviderOperationKind, String) {
    (
        record.id.0.clone(),
        completed.kind,
        completed.client_request_id_digest.clone(),
    )
}

fn acceptance_payload_fields_match(
    stored: &SponsoredGrantAcceptance,
    candidate: &SponsoredGrantAcceptance,
) -> bool {
    stored.launch_id == candidate.launch_id
        && stored.contributor == candidate.contributor
        && stored.revision_id == candidate.revision_id
        && stored.revision_digest == candidate.revision_digest
        && stored.policy_id == candidate.policy_id
        && stored.policy_generation == candidate.policy_generation
        && stored.terms_digest == candidate.terms_digest
        && stored.launch_snapshot_digest == candidate.launch_snapshot_digest
}

fn acceptance_identity_matches(
    stored: &SponsoredGrantAcceptance,
    candidates: &SponsorshipOperationalPseudonymSet,
) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.as_str() == stored.idempotency_key_hash)
}

fn require_gate(
    state: &MemoryState,
    expected: &SponsorshipGateFence,
) -> Result<(), SponsorshipStoreError> {
    if &state.gate_fence != expected || state.gate_fence.kill_switch_engaged {
        return Err(SponsorshipStoreError::GateChanged);
    }
    Ok(())
}

fn require_activation_dependencies(
    state: &MemoryState,
    intent: &GrantActivationIntent,
    launch: &SponsoredLaunchAuthority,
    acceptance: &SponsoredGrantAcceptance,
    now: &str,
) -> Result<(), SponsorshipStoreError> {
    require_gate(state, &intent.gate_fence)?;
    let stored_launch = state
        .launches
        .get(&launch.launch_id.0)
        .ok_or(SponsorshipStoreError::LaunchAuthorityChanged)?;
    if stored_launch != launch
        || launch.state != SponsoredLaunchState::Starting
        || launch.session_id.is_none()
        || launch.session_id.as_ref() != Some(&intent.session_id)
        || launch.digest != intent.launch_authority_digest
    {
        return Err(SponsorshipStoreError::LaunchAuthorityChanged);
    }
    let stored_acceptance = state
        .acceptances
        .get(&launch.launch_id.0)
        .ok_or(SponsorshipStoreError::AcceptanceChanged)?;
    if stored_acceptance != acceptance
        || acceptance.digest != intent.acceptance_digest
        || acceptance.launch_id != launch.launch_id
        || acceptance.contributor != launch.contributor
        || acceptance.revision_id != launch.revision_id
        || acceptance.revision_digest != launch.revision_digest
        || acceptance.policy_id != launch.policy_id
        || acceptance.launch_snapshot_digest != launch.launch_snapshot_digest
    {
        return Err(SponsorshipStoreError::AcceptanceChanged);
    }
    let policy = state
        .policies
        .get(&intent.policy_id.0)
        .ok_or(SponsorshipStoreError::GrantUnavailable)?;
    let (credential, credential_secret) = state
        .credentials
        .get(&policy.credential_id.0)
        .ok_or(SponsorshipStoreError::GrantUnavailable)?;
    if !policy.is_available_at(now)
        || policy.id != launch.policy_id
        || policy.tenant_id != launch.tenant_id
        || policy.generation != intent.policy_generation
        || credential.state != SponsoredCredentialState::Active
        || credential.material_state != SponsoredCredentialMaterialState::Retained
        || credential_secret.is_none()
        || credential.generation != intent.credential_generation
        || credential.owner != policy.owner
        || credential.authority.provider != policy.provider_policy.provider
        || credential.authority.provider_policy_key != policy.provider_policy.provider_policy_key
        || intent.allocation.quota != policy.per_launch_quota
        || intent.allocation.spend_micros != policy.per_launch_budget_micros
    {
        return Err(SponsorshipStoreError::GrantUnavailable);
    }
    Ok(())
}

fn require_current_terms(
    state: &MemoryState,
    intent: &GrantActivationIntent,
    acceptance: &SponsoredGrantAcceptance,
) -> Result<(), SponsorshipStoreError> {
    let policy = state
        .policies
        .get(&intent.policy_id.0)
        .ok_or(SponsorshipStoreError::GrantUnavailable)?;
    let usage = state
        .policy_usage
        .get(&intent.policy_id.0)
        .ok_or(SponsorshipStoreError::GrantUnavailable)?;
    let terms = SponsoredGrantTerms::from_policy(policy, usage)
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if acceptance.policy_generation != terms.policy_generation
        || acceptance.terms_digest != terms.digest
    {
        return Err(SponsorshipStoreError::AcceptanceChanged);
    }
    Ok(())
}

fn activation_dependencies_are_current(
    state: &MemoryState,
    intent: &GrantActivationIntent,
    now: &str,
) -> bool {
    let Some(launch) = state.launches.get(&intent.launch_id.0) else {
        return false;
    };
    let Some(acceptance) = state.acceptances.get(&intent.launch_id.0) else {
        return false;
    };
    if require_activation_dependencies(state, intent, launch, acceptance, now).is_err() {
        return false;
    }
    intent.provider_receipt.as_ref().is_none_or(|receipt| {
        let Some(now) = crate::seconds_from_rfc3339(now) else {
            return false;
        };
        crate::seconds_from_rfc3339(&receipt.issued_at).is_some_and(|issued| issued <= now)
            && crate::seconds_from_rfc3339(&receipt.expires_at).is_some_and(|expires| now < expires)
    })
}

fn reserve_activation_allocation(
    state: &mut MemoryState,
    intent: &GrantActivationIntent,
) -> Result<(), SponsorshipStoreError> {
    let policy = state
        .policies
        .get(&intent.policy_id.0)
        .cloned()
        .ok_or(SponsorshipStoreError::GrantUnavailable)?;
    let usage = state
        .policy_usage
        .get_mut(&intent.policy_id.0)
        .ok_or(SponsorshipStoreError::GrantUnavailable)?;
    if usage.active_launches >= policy.max_concurrent_launches {
        return Err(SponsorshipStoreError::ConcurrentLaunchLimit);
    }
    let exposure = usage
        .consumed
        .checked_add(&usage.reserved)
        .and_then(|value| value.checked_add(&intent.allocation))
        .ok_or(SponsorshipStoreError::Conflict)?;
    if !exposure.quota.fits_within(policy.aggregate_quota) {
        return Err(SponsorshipStoreError::AggregateQuotaExceeded);
    }
    if exposure.spend_micros > policy.aggregate_budget_micros {
        return Err(SponsorshipStoreError::AggregateBudgetExceeded);
    }
    usage.reserved = usage
        .reserved
        .checked_add(&intent.allocation)
        .ok_or(SponsorshipStoreError::Conflict)?;
    usage.active_launches += 1;
    Ok(())
}

fn release_activation_allocation(
    state: &mut MemoryState,
    intent: &GrantActivationIntent,
) -> Result<(), SponsorshipStoreError> {
    let usage = state
        .policy_usage
        .get_mut(&intent.policy_id.0)
        .ok_or(SponsorshipStoreError::GrantUnavailable)?;
    usage.reserved = usage
        .reserved
        .checked_sub(&intent.allocation)
        .ok_or(SponsorshipStoreError::Conflict)?;
    usage.active_launches = usage
        .active_launches
        .checked_sub(1)
        .ok_or(SponsorshipStoreError::Conflict)?;
    Ok(())
}

fn grant_capacity_is_released(state: SponsoredGrantState) -> bool {
    matches!(
        state,
        SponsoredGrantState::Exhausted
            | SponsoredGrantState::Expired
            | SponsoredGrantState::Revoked
            | SponsoredGrantState::Closed
    )
}

fn unresolved_usage_for_grant(
    state: &MemoryState,
    grant_id: &SponsoredGrantId,
    excluded_reservation_id: Option<&str>,
) -> Result<SponsoredUsageSummary, SponsorshipStoreError> {
    let mut held = SponsoredUsageSummary::default();
    for reservation in state.reservations.values().filter(|reservation| {
        reservation.grant_id == *grant_id
            && excluded_reservation_id != Some(reservation.id.0.as_str())
            && matches!(
                reservation.state,
                SponsoredUsageState::Reserved | SponsoredUsageState::Ambiguous
            )
    }) {
        held = add_usage(held, &reservation.reserved)?;
    }
    Ok(held)
}

fn release_terminal_grant_capacity(
    state: &mut MemoryState,
    grant: &SponsoredGrant,
    excluded_reservation_id: Option<&str>,
) -> Result<(), SponsorshipStoreError> {
    let unresolved = unresolved_usage_for_grant(state, &grant.id, excluded_reservation_id)?;
    let unused = grant
        .allocation
        .checked_sub(&grant.usage)
        .ok_or(SponsorshipStoreError::Conflict)?;
    let releasable = unused
        .checked_sub(&unresolved)
        .ok_or(SponsorshipStoreError::Conflict)?;
    let usage = state
        .policy_usage
        .get_mut(&grant.policy_id.0)
        .ok_or(SponsorshipStoreError::Conflict)?;
    usage.reserved = usage
        .reserved
        .checked_sub(&releasable)
        .ok_or(SponsorshipStoreError::Conflict)?;
    usage.active_launches = usage
        .active_launches
        .checked_sub(1)
        .ok_or(SponsorshipStoreError::Conflict)?;
    Ok(())
}

fn test_activation_lease(
    holder: String,
    now: &str,
) -> Result<GrantActivationLease, SponsorshipStoreError> {
    let acquired = crate::seconds_from_rfc3339(now)
        .ok_or_else(|| SponsorshipStoreError::Malformed("invalid database clock".into()))?;
    let expires = acquired
        .checked_add(30)
        .ok_or_else(|| SponsorshipStoreError::Malformed("database clock overflow".into()))?;
    Ok(GrantActivationLease {
        holder,
        acquired_at: now.to_string(),
        expires_at: crate::rfc3339_at(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(expires),
        ),
    })
}

fn require_receipt_matches_grant(
    receipt: &ProviderGrantReceipt,
    grant: &SponsoredGrant,
) -> Result<(), SponsorshipError> {
    let operational =
        SponsorshipOperationalPseudonymizer::new("fixture-current", vec![0x5a; 32], Vec::new())
            .unwrap();
    let grant_candidates = operational
        .pseudonymize(
            SponsorshipOperationalDomain::ProviderGrantAuthority,
            &[
                &grant.id.0,
                receipt
                    .provider_grant_id
                    .expose_for_provider_or_encryption(),
            ],
        )
        .unwrap();
    let revocation_candidates = operational
        .pseudonymize(
            SponsorshipOperationalDomain::ProviderRevocationAuthority,
            &[
                &grant.id.0,
                receipt
                    .revocation_handle
                    .expose_for_provider_or_encryption(),
            ],
        )
        .unwrap();
    if !grant_candidates
        .iter()
        .any(|candidate| candidate.as_str() == grant.authority.provider_grant_id_digest)
        || !revocation_candidates
            .iter()
            .any(|candidate| candidate.as_str() == grant.authority.revocation_handle_digest)
        || grant.authority.provider_state_version != receipt.provider_state_version
        || grant.authority.expires_at != receipt.expires_at
        || grant.authority.issued_at != receipt.issued_at
    {
        return Err(SponsorshipError::Store(SponsorshipStoreError::Malformed(
            "receipt mismatch".into(),
        )));
    }
    Ok(())
}

fn add_usage(
    left: SponsoredUsageSummary,
    right: &SponsoredUsageSummary,
) -> Result<SponsoredUsageSummary, SponsorshipStoreError> {
    Ok(SponsoredUsageSummary {
        quota: left
            .quota
            .checked_add(right.quota)
            .ok_or(SponsorshipStoreError::Conflict)?,
        spend_micros: left
            .spend_micros
            .checked_add(right.spend_micros)
            .ok_or(SponsorshipStoreError::Conflict)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderMode {
    Confirmed,
    Ambiguous,
    MalformedEvidence,
    LostResponse,
    Unavailable,
    Rejected,
}

struct FakeProvider {
    ownership_mode: Mutex<ProviderMode>,
    issue_mode: Mutex<ProviderMode>,
    rotate_mode: Mutex<ProviderMode>,
    revoke_mode: Mutex<ProviderMode>,
    ownership_calls: AtomicUsize,
    issue_calls: AtomicUsize,
    rotate_calls: AtomicUsize,
    revoke_calls: AtomicUsize,
    reconcile_calls: AtomicUsize,
    receipts: Mutex<BTreeMap<String, ProviderGrantReceipt>>,
    rotations: Mutex<BTreeMap<String, ProviderCredentialEvidence>>,
    revocations: Mutex<BTreeSet<String>>,
    operation_lineages: Mutex<BTreeMap<String, String>>,
    revoked_lineages: Mutex<BTreeSet<String>>,
    rotation_provider_credential_id: Mutex<Option<String>>,
    receipt_issued_at: Mutex<String>,
    invalid_receipt: AtomicBool,
    malformed_ownership_evidence: AtomicBool,
    block_issue: AtomicBool,
    issue_entered: Notify,
    release_issue: Notify,
    block_rotate: AtomicBool,
    rotate_entered: Notify,
    release_rotate: Notify,
    block_credential_revoke: AtomicBool,
    credential_revoke_entered: Notify,
    release_credential_revoke: Notify,
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self {
            ownership_mode: Mutex::new(ProviderMode::Confirmed),
            issue_mode: Mutex::new(ProviderMode::Confirmed),
            rotate_mode: Mutex::new(ProviderMode::Confirmed),
            revoke_mode: Mutex::new(ProviderMode::Confirmed),
            ownership_calls: AtomicUsize::new(0),
            issue_calls: AtomicUsize::new(0),
            rotate_calls: AtomicUsize::new(0),
            revoke_calls: AtomicUsize::new(0),
            reconcile_calls: AtomicUsize::new(0),
            receipts: Mutex::new(BTreeMap::new()),
            rotations: Mutex::new(BTreeMap::new()),
            revocations: Mutex::new(BTreeSet::new()),
            operation_lineages: Mutex::new(BTreeMap::new()),
            revoked_lineages: Mutex::new(BTreeSet::new()),
            rotation_provider_credential_id: Mutex::new(None),
            receipt_issued_at: Mutex::new(NOW.into()),
            invalid_receipt: AtomicBool::new(false),
            malformed_ownership_evidence: AtomicBool::new(false),
            block_issue: AtomicBool::new(false),
            issue_entered: Notify::new(),
            release_issue: Notify::new(),
            block_rotate: AtomicBool::new(false),
            rotate_entered: Notify::new(),
            release_rotate: Notify::new(),
            block_credential_revoke: AtomicBool::new(false),
            credential_revoke_entered: Notify::new(),
            release_credential_revoke: Notify::new(),
        }
    }
}

#[async_trait]
impl SponsoredCredentialProvider for FakeProvider {
    async fn validate_ownership(
        &self,
        _owner: &SponsorBillingOwner,
        provider: &str,
        provider_policy_key: &str,
        credential: &ProviderCredentialSecret,
    ) -> Result<ProviderCredentialEvidence, SponsoredProviderError> {
        self.ownership_calls.fetch_add(1, Ordering::SeqCst);
        if self.malformed_ownership_evidence.load(Ordering::SeqCst) {
            return Err(SponsoredProviderError::MalformedEvidence);
        }
        match *self.ownership_mode.lock().await {
            ProviderMode::Confirmed => {}
            ProviderMode::Unavailable => return Err(SponsoredProviderError::Unavailable),
            ProviderMode::Ambiguous | ProviderMode::LostResponse => {
                return Err(SponsoredProviderError::Ambiguous)
            }
            ProviderMode::MalformedEvidence => {
                return Err(SponsoredProviderError::MalformedEvidence)
            }
            ProviderMode::Rejected => return Err(SponsoredProviderError::Rejected),
        }
        let provider_credential_id = match credential.expose() {
            SECRET | "replacement-credential" => "external-credential-id",
            "other-provider-credential" => "other-external-credential-id",
            _ => return Err(SponsoredProviderError::Rejected),
        };
        Ok(ProviderCredentialEvidence {
            provider: provider.to_string(),
            provider_policy_key: provider_policy_key.to_string(),
            provider_policy_handle: SensitiveProviderValue::new("internal-policy-handle").unwrap(),
            provider_credential_id: SensitiveProviderValue::new(provider_credential_id).unwrap(),
            state_version: 1,
            ownership_verified_at: NOW.into(),
            supports_expiry: true,
            supports_revocation: true,
        })
    }

    async fn issue_grant(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        _policy: &SponsorshipPolicy,
        _launch_id: &LaunchId,
        _session_id: &SessionId,
    ) -> Result<ProviderGrantReceipt, SponsoredProviderError> {
        assert!(matches!(
            secret.expose(),
            SECRET | "replacement-credential" | "replacement-credential-2"
        ));
        self.issue_calls.fetch_add(1, Ordering::SeqCst);
        if self.block_issue.load(Ordering::SeqCst) {
            self.issue_entered.notify_one();
            self.release_issue.notified().await;
        }
        let mode = *self.issue_mode.lock().await;
        match mode {
            ProviderMode::Ambiguous => return Err(SponsoredProviderError::Ambiguous),
            ProviderMode::MalformedEvidence => {
                return Err(SponsoredProviderError::MalformedEvidence)
            }
            ProviderMode::Unavailable => return Err(SponsoredProviderError::Unavailable),
            ProviderMode::Rejected => return Err(SponsoredProviderError::Rejected),
            ProviderMode::Confirmed | ProviderMode::LostResponse => {}
        }
        if self
            .revoked_lineages
            .lock()
            .await
            .contains(&credential.id.0)
        {
            return Err(SponsoredProviderError::Rejected);
        }
        let issued_at = self.receipt_issued_at.lock().await.clone();
        let provider_state_version = if self.invalid_receipt.load(Ordering::SeqCst) {
            0
        } else {
            1
        };
        let mut receipts = self.receipts.lock().await;
        let receipt = receipts
            .entry(stable_request_id.to_string())
            .or_insert_with(|| ProviderGrantReceipt {
                provider_grant_id: SensitiveProviderValue::new(format!(
                    "provider-grant-{stable_request_id}"
                ))
                .unwrap(),
                provider_state_version,
                expires_at: GRANT_EXPIRY.into(),
                revocation_handle: SensitiveProviderValue::new(format!(
                    "provider-revoke-{stable_request_id}"
                ))
                .unwrap(),
                issued_at,
            })
            .clone();
        self.operation_lineages
            .lock()
            .await
            .insert(stable_request_id.to_string(), credential.id.0.clone());
        if mode == ProviderMode::LostResponse {
            Err(SponsoredProviderError::Ambiguous)
        } else {
            Ok(receipt)
        }
    }

    async fn reconcile_grant_issue(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        _secret: &ProviderCredentialSecret,
        _policy: &SponsorshipPolicy,
        _launch_id: &LaunchId,
        _session_id: &SessionId,
    ) -> Result<ProviderOperationResolution<ProviderGrantReceipt>, SponsoredProviderError> {
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        if *self.issue_mode.lock().await == ProviderMode::MalformedEvidence {
            return Err(SponsoredProviderError::MalformedEvidence);
        }
        if self
            .revoked_lineages
            .lock()
            .await
            .contains(&credential.id.0)
        {
            return Ok(ProviderOperationResolution::Rejected);
        }
        if let Some(receipt) = self.receipts.lock().await.get(stable_request_id).cloned() {
            return Ok(ProviderOperationResolution::Confirmed(receipt));
        }
        Ok(match *self.issue_mode.lock().await {
            ProviderMode::Rejected => ProviderOperationResolution::Rejected,
            ProviderMode::Confirmed
            | ProviderMode::Ambiguous
            | ProviderMode::MalformedEvidence
            | ProviderMode::LostResponse
            | ProviderMode::Unavailable => ProviderOperationResolution::Pending,
        })
    }

    async fn revoke_uncommitted_grant(
        &self,
        stable_request_id: &str,
        _receipt: &ProviderGrantReceipt,
        _credential: &SponsoredCredentialRecord,
        _secret: &ProviderCredentialSecret,
    ) -> Result<(), SponsoredProviderError> {
        self.revoke_calls.fetch_add(1, Ordering::SeqCst);
        let mode = *self.revoke_mode.lock().await;
        match mode {
            ProviderMode::Confirmed | ProviderMode::LostResponse => {
                self.revocations
                    .lock()
                    .await
                    .insert(stable_request_id.to_string());
                if mode == ProviderMode::LostResponse {
                    Err(SponsoredProviderError::Ambiguous)
                } else {
                    Ok(())
                }
            }
            ProviderMode::Ambiguous => Err(SponsoredProviderError::Ambiguous),
            ProviderMode::MalformedEvidence => Err(SponsoredProviderError::MalformedEvidence),
            ProviderMode::Unavailable => Err(SponsoredProviderError::Unavailable),
            ProviderMode::Rejected => Err(SponsoredProviderError::Rejected),
        }
    }

    async fn reconcile_uncommitted_grant_revocation(
        &self,
        stable_request_id: &str,
        _receipt: &ProviderGrantReceipt,
        _credential: &SponsoredCredentialRecord,
        _secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError> {
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        if *self.revoke_mode.lock().await == ProviderMode::MalformedEvidence {
            return Err(SponsoredProviderError::MalformedEvidence);
        }
        if self.revocations.lock().await.contains(stable_request_id) {
            return Ok(ProviderOperationResolution::Confirmed(()));
        }
        Ok(match *self.revoke_mode.lock().await {
            ProviderMode::Rejected => ProviderOperationResolution::Rejected,
            ProviderMode::Confirmed
            | ProviderMode::Ambiguous
            | ProviderMode::MalformedEvidence
            | ProviderMode::LostResponse
            | ProviderMode::Unavailable => ProviderOperationResolution::Pending,
        })
    }

    async fn rotate_credential(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        current_secret: &ProviderCredentialSecret,
        replacement_secret: &ProviderCredentialSecret,
    ) -> Result<ProviderCredentialEvidence, SponsoredProviderError> {
        assert!(matches!(
            current_secret.expose(),
            SECRET | "replacement-credential" | "replacement-credential-2"
        ));
        assert!(matches!(
            replacement_secret.expose(),
            "replacement-credential" | "replacement-credential-2"
        ));
        self.rotate_calls.fetch_add(1, Ordering::SeqCst);
        if self.block_rotate.load(Ordering::SeqCst) {
            self.rotate_entered.notify_one();
            self.release_rotate.notified().await;
        }
        let mode = *self.rotate_mode.lock().await;
        if self
            .revoked_lineages
            .lock()
            .await
            .contains(&credential.id.0)
        {
            return Err(SponsoredProviderError::Rejected);
        }
        match mode {
            ProviderMode::Ambiguous => Err(SponsoredProviderError::Ambiguous),
            ProviderMode::MalformedEvidence => Err(SponsoredProviderError::MalformedEvidence),
            ProviderMode::Unavailable => Err(SponsoredProviderError::Unavailable),
            ProviderMode::Rejected => Err(SponsoredProviderError::Rejected),
            ProviderMode::Confirmed | ProviderMode::LostResponse => {
                let provider_credential_id = self
                    .rotation_provider_credential_id
                    .lock()
                    .await
                    .clone()
                    .unwrap_or_else(|| {
                        format!("replacement-external-id:{}", replacement_secret.expose())
                    });
                let evidence = ProviderCredentialEvidence {
                    provider: credential.authority.provider.clone(),
                    provider_policy_key: credential.authority.provider_policy_key.clone(),
                    provider_policy_handle: credential.authority.provider_policy_handle.clone(),
                    provider_credential_id: SensitiveProviderValue::new(provider_credential_id)
                        .unwrap(),
                    state_version: credential.authority.state_version + 1,
                    ownership_verified_at: LATER.into(),
                    supports_expiry: credential.authority.supports_expiry,
                    supports_revocation: credential.authority.supports_revocation,
                };
                self.rotations
                    .lock()
                    .await
                    .insert(stable_request_id.to_string(), evidence.clone());
                if mode == ProviderMode::LostResponse {
                    Err(SponsoredProviderError::Ambiguous)
                } else {
                    Ok(evidence)
                }
            }
        }
    }

    async fn reconcile_credential_rotation(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        _current_secret: &ProviderCredentialSecret,
        _replacement_secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<ProviderCredentialEvidence>, SponsoredProviderError>
    {
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        if *self.rotate_mode.lock().await == ProviderMode::MalformedEvidence {
            return Err(SponsoredProviderError::MalformedEvidence);
        }
        if self
            .revoked_lineages
            .lock()
            .await
            .contains(&credential.id.0)
        {
            return Ok(ProviderOperationResolution::Rejected);
        }
        if let Some(evidence) = self.rotations.lock().await.get(stable_request_id).cloned() {
            return Ok(ProviderOperationResolution::Confirmed(evidence));
        }
        Ok(match *self.rotate_mode.lock().await {
            ProviderMode::Rejected => ProviderOperationResolution::Rejected,
            ProviderMode::Confirmed
            | ProviderMode::Ambiguous
            | ProviderMode::MalformedEvidence
            | ProviderMode::LostResponse
            | ProviderMode::Unavailable => ProviderOperationResolution::Pending,
        })
    }

    async fn revoke_credential(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        _secret: &ProviderCredentialSecret,
        staged_replacement: Option<&ProviderCredentialSecret>,
        superseded_rotation: Option<&PendingProviderOperation>,
    ) -> Result<(), SponsoredProviderError> {
        assert_eq!(staged_replacement.is_some(), superseded_rotation.is_some());
        if let Some(replacement) = staged_replacement {
            assert!(matches!(
                replacement.expose(),
                "replacement-credential" | "replacement-credential-2"
            ));
            assert_eq!(
                superseded_rotation.unwrap().kind,
                ProviderOperationKind::RotateCredential
            );
        }
        self.revoke_calls.fetch_add(1, Ordering::SeqCst);
        if self.block_credential_revoke.load(Ordering::SeqCst) {
            self.credential_revoke_entered.notify_one();
            self.release_credential_revoke.notified().await;
        }
        let mode = *self.revoke_mode.lock().await;
        match mode {
            ProviderMode::Confirmed | ProviderMode::LostResponse => {
                if let Some(rotation) = superseded_rotation {
                    self.rotations
                        .lock()
                        .await
                        .remove(&rotation.provider_request_id);
                }
                self.revocations
                    .lock()
                    .await
                    .insert(stable_request_id.to_string());
                self.revoked_lineages
                    .lock()
                    .await
                    .insert(credential.id.0.clone());
                if mode == ProviderMode::LostResponse {
                    Err(SponsoredProviderError::Ambiguous)
                } else {
                    Ok(())
                }
            }
            ProviderMode::Ambiguous => Err(SponsoredProviderError::Ambiguous),
            ProviderMode::MalformedEvidence => Err(SponsoredProviderError::MalformedEvidence),
            ProviderMode::Unavailable => Err(SponsoredProviderError::Unavailable),
            ProviderMode::Rejected => Err(SponsoredProviderError::Rejected),
        }
    }

    async fn reconcile_credential_revocation(
        &self,
        stable_request_id: &str,
        credential: &SponsoredCredentialRecord,
        _secret: &ProviderCredentialSecret,
        staged_replacement: Option<&ProviderCredentialSecret>,
        superseded_rotation: Option<&PendingProviderOperation>,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError> {
        assert_eq!(staged_replacement.is_some(), superseded_rotation.is_some());
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        if *self.revoke_mode.lock().await == ProviderMode::MalformedEvidence {
            return Err(SponsoredProviderError::MalformedEvidence);
        }
        if self.revocations.lock().await.contains(stable_request_id)
            || self
                .revoked_lineages
                .lock()
                .await
                .contains(&credential.id.0)
        {
            return Ok(ProviderOperationResolution::Confirmed(()));
        }
        Ok(match *self.revoke_mode.lock().await {
            ProviderMode::Rejected => ProviderOperationResolution::Rejected,
            ProviderMode::Confirmed
            | ProviderMode::Ambiguous
            | ProviderMode::MalformedEvidence
            | ProviderMode::LostResponse
            | ProviderMode::Unavailable => ProviderOperationResolution::Pending,
        })
    }

    async fn revoke_grant(
        &self,
        stable_request_id: &str,
        _grant: &SponsoredGrant,
        _provider_receipt: &ProviderGrantReceipt,
        _credential: &SponsoredCredentialRecord,
        _secret: &ProviderCredentialSecret,
    ) -> Result<(), SponsoredProviderError> {
        self.revoke_calls.fetch_add(1, Ordering::SeqCst);
        let mode = *self.revoke_mode.lock().await;
        match mode {
            ProviderMode::Confirmed | ProviderMode::LostResponse => {
                self.revocations
                    .lock()
                    .await
                    .insert(stable_request_id.to_string());
                if mode == ProviderMode::LostResponse {
                    Err(SponsoredProviderError::Ambiguous)
                } else {
                    Ok(())
                }
            }
            ProviderMode::Ambiguous => Err(SponsoredProviderError::Ambiguous),
            ProviderMode::MalformedEvidence => Err(SponsoredProviderError::MalformedEvidence),
            ProviderMode::Unavailable => Err(SponsoredProviderError::Unavailable),
            ProviderMode::Rejected => Err(SponsoredProviderError::Rejected),
        }
    }

    async fn reconcile_grant_revocation(
        &self,
        stable_request_id: &str,
        _grant: &SponsoredGrant,
        _provider_receipt: &ProviderGrantReceipt,
        _credential: &SponsoredCredentialRecord,
        _secret: &ProviderCredentialSecret,
    ) -> Result<ProviderOperationResolution<()>, SponsoredProviderError> {
        self.reconcile_calls.fetch_add(1, Ordering::SeqCst);
        if *self.revoke_mode.lock().await == ProviderMode::MalformedEvidence {
            return Err(SponsoredProviderError::MalformedEvidence);
        }
        if self.revocations.lock().await.contains(stable_request_id) {
            return Ok(ProviderOperationResolution::Confirmed(()));
        }
        Ok(match *self.revoke_mode.lock().await {
            ProviderMode::Rejected => ProviderOperationResolution::Rejected,
            ProviderMode::Confirmed
            | ProviderMode::Ambiguous
            | ProviderMode::MalformedEvidence
            | ProviderMode::LostResponse
            | ProviderMode::Unavailable => ProviderOperationResolution::Pending,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BillingMode {
    Confirmed,
    Rejected,
    Ambiguous,
    LostResponse,
    Unavailable,
}

struct FakeBilling {
    mode: Mutex<BillingMode>,
    calls: AtomicUsize,
    eligibility_observations: Mutex<BTreeSet<String>>,
    block_confirm: AtomicBool,
    confirm_entered: Notify,
    release_confirm: Notify,
}

impl Default for FakeBilling {
    fn default() -> Self {
        Self {
            mode: Mutex::new(BillingMode::Confirmed),
            calls: AtomicUsize::new(0),
            eligibility_observations: Mutex::new(BTreeSet::new()),
            block_confirm: AtomicBool::new(false),
            confirm_entered: Notify::new(),
            release_confirm: Notify::new(),
        }
    }
}

#[async_trait]
impl SponsoredBillingProvider for FakeBilling {
    async fn confirm_grant(
        &self,
        stable_request_id: &str,
        _policy: &SponsorshipPolicy,
        _launch_id: &LaunchId,
    ) -> Result<BillingAuthorization, SponsoredBillingError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.block_confirm.load(Ordering::SeqCst) {
            self.confirm_entered.notify_one();
            self.release_confirm.notified().await;
        }
        match *self.mode.lock().await {
            BillingMode::Confirmed => {
                self.eligibility_observations
                    .lock()
                    .await
                    .insert(stable_request_id.to_string());
                Ok(BillingAuthorization::Confirmed)
            }
            BillingMode::Rejected => Ok(BillingAuthorization::Rejected),
            BillingMode::Ambiguous => Err(SponsoredBillingError::Ambiguous),
            BillingMode::LostResponse => {
                self.eligibility_observations
                    .lock()
                    .await
                    .insert(stable_request_id.to_string());
                Err(SponsoredBillingError::Ambiguous)
            }
            BillingMode::Unavailable => Err(SponsoredBillingError::Unavailable),
        }
    }

    async fn reconcile_grant(
        &self,
        stable_request_id: &str,
        _policy: &SponsorshipPolicy,
        _launch_id: &LaunchId,
    ) -> Result<ProviderOperationResolution<BillingAuthorization>, SponsoredBillingError> {
        if self
            .eligibility_observations
            .lock()
            .await
            .contains(stable_request_id)
        {
            return Ok(ProviderOperationResolution::Confirmed(
                BillingAuthorization::Confirmed,
            ));
        }
        Ok(match *self.mode.lock().await {
            BillingMode::Rejected => {
                ProviderOperationResolution::Confirmed(BillingAuthorization::Rejected)
            }
            BillingMode::Confirmed
            | BillingMode::Ambiguous
            | BillingMode::LostResponse
            | BillingMode::Unavailable => ProviderOperationResolution::Pending,
        })
    }
}

struct FakeClock {
    now: RwLock<String>,
}

impl FakeClock {
    fn new(now: &str) -> Self {
        Self {
            now: RwLock::new(now.to_string()),
        }
    }

    fn set(&self, now: &str) {
        *self.now.write().unwrap() = now.to_string();
    }
}

impl SponsorshipClock for FakeClock {
    fn now(&self) -> String {
        self.now.read().unwrap().clone()
    }
}

#[derive(Default)]
struct FakePricing {
    quote_calls: AtomicUsize,
    quote_error: Mutex<Option<SponsoredPricingError>>,
    actual_calls: AtomicUsize,
    actual_error: Mutex<Option<SponsoredPricingError>>,
    actual_quota_override: Mutex<Option<SponsoredQuota>>,
}

#[async_trait]
impl SponsoredPricingProvider for FakePricing {
    async fn quote(
        &self,
        provider: &str,
        model: &str,
        resource: &str,
        maximum_quota: SponsoredQuota,
        _observed_at: &str,
    ) -> Result<SponsoredPriceQuote, SponsoredPricingError> {
        self.quote_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.quote_error.lock().await.clone() {
            return Err(error);
        }
        let mut quote = SponsoredPriceQuote {
            provider: provider.to_string(),
            model: model.to_string(),
            resource: resource.to_string(),
            maximum: SponsoredUsageSummary {
                quota: maximum_quota,
                spend_micros: maximum_quota.requests.saturating_mul(100),
            },
            pricing_generation: 1,
            digest: String::new(),
        };
        quote.digest = quote
            .calculate_digest()
            .map_err(|_| SponsoredPricingError::Malformed)?;
        Ok(quote)
    }

    async fn price_actual(
        &self,
        quote: &SponsoredPriceQuote,
        actual_quota: SponsoredQuota,
        _observed_at: &str,
    ) -> Result<SponsoredUsageSummary, SponsoredPricingError> {
        self.actual_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.actual_error.lock().await.clone() {
            return Err(error);
        }
        if !actual_quota.fits_within(quote.maximum.quota) {
            return Err(SponsoredPricingError::Malformed);
        }
        let returned_quota = self
            .actual_quota_override
            .lock()
            .await
            .unwrap_or(actual_quota);
        Ok(SponsoredUsageSummary {
            quota: returned_quota,
            spend_micros: returned_quota.requests.saturating_mul(100),
        })
    }
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sensitive_test_digest(domain: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn subject() -> SubjectId {
    SubjectId("sponsor-owner".into())
}

fn contributor() -> SubjectId {
    SubjectId("launch-contributor".into())
}

fn owner() -> SponsorBillingOwner {
    SponsorBillingOwner::Developer {
        subject: subject(),
        display_name: "Example Sponsor".into(),
    }
}

fn enabled_gate() -> SponsorshipGate {
    SponsorshipGate {
        enabled: true,
        expiry_evidence_id: Some("expiry-evidence-524".into()),
        revocation_evidence_id: Some("revocation-evidence-445".into()),
        allowed_provider_policy_keys: BTreeSet::from([
            POLICY_KEY.into(),
            ALTERNATE_POLICY_KEY.into(),
        ]),
        maximum_policy_budget_micros: 1_000_000,
        maximum_policy_requests: 1_000,
        maximum_concurrent_launches: 10,
        kill_switch_engaged: false,
    }
}

mod current_tests {
    use super::*;

    #[test]
    fn memory_recovery_cursors_are_collection_bound() {
        let rows = vec![
            (NOW.to_string(), "candidate-a".to_string()),
            (LATER.to_string(), "candidate-b".to_string()),
        ];
        let first = recovery_memory_page(
            rows.clone(),
            &SponsorshipRecoveryPageRequest::new(1, None),
            "credential_recovery",
            |row| (&row.0, &row.1),
        )
        .unwrap();
        assert_eq!(first.items, vec![rows[0].clone()]);
        let cursor = first.next.expect("the second row continues the page");
        assert!(matches!(
            recovery_memory_page(
                rows,
                &SponsorshipRecoveryPageRequest::new(1, Some(cursor)),
                "usage_recovery",
                |row| (&row.0, &row.1),
            ),
            Err(SponsorshipStoreError::Malformed(_))
        ));
    }

    #[derive(Clone, Copy)]
    struct Limits {
        per_launch_quota: SponsoredQuota,
        aggregate_quota: SponsoredQuota,
        per_launch_budget_micros: u64,
        aggregate_budget_micros: u64,
        max_requests_per_minute: u32,
        max_concurrent_launches: u32,
    }

    impl Default for Limits {
        fn default() -> Self {
            Self {
                per_launch_quota: SponsoredQuota {
                    requests: 4,
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                },
                aggregate_quota: SponsoredQuota {
                    requests: 8,
                    input_tokens: 2_000,
                    output_tokens: 2_000,
                },
                per_launch_budget_micros: 400,
                aggregate_budget_micros: 800,
                max_requests_per_minute: 10,
                max_concurrent_launches: 4,
            }
        }
    }

    struct Harness {
        service: Arc<SponsorshipService>,
        store: Arc<MemoryStore>,
        provider: Arc<FakeProvider>,
        billing: Arc<FakeBilling>,
        pricing: Arc<FakePricing>,
        clock: Arc<FakeClock>,
        audit: Arc<SponsorshipAuditPseudonymizer>,
        operational: Arc<SponsorshipOperationalPseudonymizer>,
        sponsor: Caller,
        contributor: Caller,
        meter: Caller,
        admin: Caller,
        policy: SponsorshipPolicy,
        terms: SponsoredGrantTerms,
        launch_id: LaunchId,
        session_id: SessionId,
    }

    fn caller(subject: SubjectId, capabilities: impl IntoIterator<Item = Capability>) -> Caller {
        Caller::new(subject, Authorization::from_capabilities(capabilities))
    }

    fn sponsor_caller() -> Caller {
        caller(subject(), [Capability::ManageOwnSponsorships])
    }

    fn contributor_caller() -> Caller {
        caller(contributor(), [Capability::UseOwnSessions])
    }

    fn meter_caller() -> Caller {
        caller(
            SubjectId("trusted-sponsored-meter".into()),
            [Capability::ReconcileSponsoredUsage],
        )
    }

    fn admin_caller() -> Caller {
        caller(
            SubjectId("platform-administrator".into()),
            [Capability::ConfigurePlatform],
        )
    }

    fn launch_authority(
        policy: &SponsorshipPolicy,
        launch_id: LaunchId,
        contributor: SubjectId,
    ) -> SponsoredLaunchAuthority {
        let mut launch = SponsoredLaunchAuthority {
            tenant_id: policy.tenant_id.clone(),
            launch_id,
            contributor,
            revision_id: TriggerRevisionId::new(),
            revision_digest: digest(b"fixture-revision"),
            launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
            policy_id: policy.id.clone(),
            session_id: None,
            state: SponsoredLaunchState::TermsReview,
            generation: 1,
            digest: String::new(),
        };
        launch.digest = launch.calculate_digest().unwrap();
        launch
    }

    async fn harness(limits: Limits) -> Harness {
        let gate = enabled_gate();
        let store = Arc::new(MemoryStore::default());
        store.configure_gate(&gate).await;
        let provider = Arc::new(FakeProvider::default());
        let billing = Arc::new(FakeBilling::default());
        let pricing = Arc::new(FakePricing::default());
        let clock = Arc::new(FakeClock::new(NOW));
        let audit = Arc::new(
            SponsorshipAuditPseudonymizer::new(vec![0xa5; 32]).expect("valid fixture audit key"),
        );
        let operational = Arc::new(
            SponsorshipOperationalPseudonymizer::new("fixture-current", vec![0x5a; 32], Vec::new())
                .expect("valid fixture operational key"),
        );
        let service = Arc::new(SponsorshipService::new(
            gate,
            store.clone(),
            provider.clone(),
            billing.clone(),
            pricing.clone(),
            clock.clone(),
            audit.clone(),
            operational.clone(),
        ));
        let sponsor = sponsor_caller();
        let contributor = contributor_caller();
        let meter = meter_caller();
        let admin = admin_caller();
        let credential = service
            .register_credential(
                &sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "register-credential-a".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new(SECRET).unwrap(),
                },
            )
            .await
            .unwrap();
        let policy = service
            .create_policy(
                &sponsor,
                CreateSponsorshipPolicy {
                    tenant_id: "tenant-a".into(),
                    owner: owner(),
                    credential_id: credential.id,
                    provider_policy: SponsoredProviderPolicy {
                        provider: PROVIDER.into(),
                        provider_policy_key: POLICY_KEY.into(),
                        allowed_models: BTreeSet::from([MODEL.into()]),
                        allowed_resources: BTreeSet::from([RESOURCE.into()]),
                    },
                    per_launch_quota: limits.per_launch_quota,
                    aggregate_quota: limits.aggregate_quota,
                    per_launch_budget_micros: limits.per_launch_budget_micros,
                    aggregate_budget_micros: limits.aggregate_budget_micros,
                    max_requests_per_minute: limits.max_requests_per_minute,
                    max_concurrent_launches: limits.max_concurrent_launches,
                    expires_at: POLICY_EXPIRY.into(),
                },
            )
            .await
            .unwrap();
        let launch_id = LaunchId::new();
        let launch = launch_authority(&policy, launch_id.clone(), contributor.subject.clone());
        store.put_launch(launch).await;
        let terms = service.terms(&contributor, &launch_id).await.unwrap();
        Harness {
            service,
            store,
            provider,
            billing,
            pricing,
            clock,
            audit,
            operational,
            sponsor,
            contributor,
            meter,
            admin,
            policy,
            terms,
            launch_id,
            session_id: SessionId("sesn-sponsored-a".into()),
        }
    }

    async fn default_harness() -> Harness {
        harness(Limits::default()).await
    }

    fn restarted_service(harness: &Harness) -> SponsorshipService {
        service_with_operational(harness, harness.operational.clone())
    }

    fn service_with_operational(
        harness: &Harness,
        operational: Arc<SponsorshipOperationalPseudonymizer>,
    ) -> SponsorshipService {
        SponsorshipService::new(
            harness.service.gate().clone(),
            harness.store.clone(),
            harness.provider.clone(),
            harness.billing.clone(),
            harness.pricing.clone(),
            harness.clock.clone(),
            harness.audit.clone(),
            operational,
        )
    }

    async fn accept(harness: &Harness, idempotency_key: &str) -> SponsoredGrantAcceptance {
        harness
            .service
            .accept_terms(
                &harness.contributor,
                AcceptSponsoredTerms {
                    launch_id: harness.launch_id.clone(),
                    policy_id: harness.policy.id.clone(),
                    terms_digest: harness.terms.digest.clone(),
                    launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                    idempotency_key: idempotency_key.into(),
                },
            )
            .await
            .unwrap()
    }

    async fn prepare_activation(harness: &Harness) {
        accept(harness, "acceptance-request-a").await;
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Starting,
                Some(harness.session_id.clone()),
            )
            .await;
    }

    async fn prepare_additional_activation(
        harness: &Harness,
        suffix: &str,
    ) -> (LaunchId, SessionId) {
        let launch_id = LaunchId::new();
        let session_id = SessionId(format!("session-{suffix}"));
        let launch = launch_authority(&harness.policy, launch_id.clone(), contributor());
        harness.store.put_launch(launch).await;
        let terms = harness
            .service
            .terms(&harness.contributor, &launch_id)
            .await
            .unwrap();
        harness
            .service
            .accept_terms(
                &harness.contributor,
                AcceptSponsoredTerms {
                    launch_id: launch_id.clone(),
                    policy_id: harness.policy.id.clone(),
                    terms_digest: terms.digest,
                    launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                    idempotency_key: format!("accept-additional-{suffix}"),
                },
            )
            .await
            .unwrap();
        harness
            .store
            .update_launch_state(
                &launch_id,
                SponsoredLaunchState::Starting,
                Some(session_id.clone()),
            )
            .await;
        (launch_id, session_id)
    }

    async fn activate(harness: &Harness, stable_request_id: &str) -> SponsoredGrant {
        harness
            .service
            .activate_grant(
                &harness.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: stable_request_id.into(),
                    launch_id: harness.launch_id.clone(),
                },
            )
            .await
            .unwrap()
    }

    async fn activate_for_usage(harness: &Harness) -> SponsoredGrant {
        prepare_activation(harness).await;
        let grant = activate(harness, "grant-request-a").await;
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Active,
                Some(harness.session_id.clone()),
            )
            .await;
        grant
    }

    fn usage_command(
        grant: &SponsoredGrant,
        stable_request_id: impl Into<String>,
        quota: SponsoredQuota,
    ) -> RequestSponsoredUsage {
        RequestSponsoredUsage {
            stable_request_id: stable_request_id.into(),
            grant_id: grant.id.clone(),
            provider: PROVIDER.into(),
            model: MODEL.into(),
            resource: RESOURCE.into(),
            maximum_quota: quota,
            capability: grant.capability.clone(),
        }
    }

    #[tokio::test]
    async fn authenticated_launch_happy_path_reserves_full_allocation_once() {
        let harness = default_harness().await;
        prepare_activation(&harness).await;
        let first = activate(&harness, "grant-request-happy").await;
        let second = activate(&harness, "grant-request-happy").await;
        assert_eq!(first, second);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.billing.calls.load(Ordering::SeqCst), 1);

        let state = harness.store.state.lock().await;
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, first.allocation);
        assert_eq!(usage.consumed, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 1);
        assert_eq!(state.grants.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_activation_has_one_external_mutation_and_one_grant() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        let mut tasks = Vec::new();
        for index in 0..16 {
            let harness = harness.clone();
            tasks.push(tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: format!("concurrent-grant-{index}"),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            }));
        }
        for result in futures_from_tasks(tasks).await {
            assert!(matches!(
                result,
                Ok(_) | Err(SponsorshipError::ActivationInProgress)
            ));
        }
        let grant = activate(&harness, "concurrent-grant-final").await;
        assert_eq!(grant.launch_id, harness.launch_id);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.billing.calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.store.snapshot().await.grants.len(), 1);
    }

    async fn futures_from_tasks(
        tasks: Vec<tokio::task::JoinHandle<Result<SponsoredGrant, SponsorshipError>>>,
    ) -> Vec<Result<SponsoredGrant, SponsorshipError>> {
        let mut results = Vec::with_capacity(tasks.len());
        for task in tasks {
            results.push(task.await.unwrap());
        }
        results
    }

    #[tokio::test]
    async fn lost_billing_and_provider_responses_reconcile_without_reissuing() {
        let billing_harness = default_harness().await;
        prepare_activation(&billing_harness).await;
        *billing_harness.billing.mode.lock().await = BillingMode::LostResponse;
        let error = billing_harness
            .service
            .activate_grant(
                &billing_harness.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "lost-billing-response".into(),
                    launch_id: billing_harness.launch_id.clone(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, SponsorshipError::BillingAmbiguous);
        let grant = activate(&billing_harness, "lost-billing-response").await;
        assert_eq!(grant.launch_id, billing_harness.launch_id);
        assert_eq!(billing_harness.billing.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            billing_harness.provider.issue_calls.load(Ordering::SeqCst),
            1
        );

        let provider_harness = default_harness().await;
        prepare_activation(&provider_harness).await;
        *provider_harness.provider.issue_mode.lock().await = ProviderMode::LostResponse;
        let error = provider_harness
            .service
            .activate_grant(
                &provider_harness.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "lost-provider-response".into(),
                    launch_id: provider_harness.launch_id.clone(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, SponsorshipError::ProviderAmbiguous);
        let grant = activate(&provider_harness, "lost-provider-response").await;
        assert_eq!(grant.launch_id, provider_harness.launch_id);
        assert_eq!(
            provider_harness.provider.issue_calls.load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            provider_harness
                .provider
                .reconcile_calls
                .load(Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn definitive_activation_failures_release_capacity_but_ambiguity_retains_it() {
        let billing_denied = default_harness().await;
        prepare_activation(&billing_denied).await;
        *billing_denied.billing.mode.lock().await = BillingMode::Rejected;
        let error = billing_denied
            .service
            .activate_grant(
                &billing_denied.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "definitive-billing-denial".into(),
                    launch_id: billing_denied.launch_id.clone(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, SponsorshipError::BillingRejected);
        let denied_state = billing_denied.store.state.lock().await;
        let usage = denied_state
            .policy_usage
            .get(&billing_denied.policy.id.0)
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        drop(denied_state);

        let provider_denied = default_harness().await;
        prepare_activation(&provider_denied).await;
        *provider_denied.provider.issue_mode.lock().await = ProviderMode::Rejected;
        for stable_request_id in ["definitive-provider-denial", "provider-denial-retry"] {
            assert_eq!(
                provider_denied
                    .service
                    .activate_grant(
                        &provider_denied.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: stable_request_id.into(),
                            launch_id: provider_denied.launch_id.clone(),
                        },
                    )
                    .await
                    .unwrap_err(),
                SponsorshipError::ProviderRejected
            );
        }
        assert_eq!(
            provider_denied.provider.issue_calls.load(Ordering::SeqCst),
            1
        );
        assert_eq!(provider_denied.billing.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            provider_denied.provider.revoke_calls.load(Ordering::SeqCst),
            0
        );
        let denied_state = provider_denied.store.state.lock().await;
        let intent = denied_state
            .activations
            .get(&provider_denied.launch_id.0)
            .unwrap();
        assert_eq!(intent.state, GrantActivationState::Denied);
        assert_eq!(intent.denial, Some(GrantActivationDenial::ProviderRejected));
        let billing_request_id = intent.billing_request_id.clone();
        let usage = denied_state
            .policy_usage
            .get(&provider_denied.policy.id.0)
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        drop(denied_state);
        assert!(provider_denied
            .billing
            .eligibility_observations
            .lock()
            .await
            .contains(&billing_request_id));

        let ambiguous = default_harness().await;
        prepare_activation(&ambiguous).await;
        *ambiguous.provider.issue_mode.lock().await = ProviderMode::Ambiguous;
        let error = ambiguous
            .service
            .activate_grant(
                &ambiguous.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "ambiguous-provider-issue".into(),
                    launch_id: ambiguous.launch_id.clone(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error, SponsorshipError::ProviderAmbiguous);
        let ambiguous_state = ambiguous.store.state.lock().await;
        let usage = ambiguous_state
            .policy_usage
            .get(&ambiguous.policy.id.0)
            .unwrap();
        assert_eq!(usage.reserved.quota, ambiguous.policy.per_launch_quota);
        assert_eq!(usage.active_launches, 1);
    }

    #[tokio::test]
    async fn activation_commit_rechecks_the_durable_launch_authority() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        let worker = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "launch-fence-race".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Active,
                Some(harness.session_id.clone()),
            )
            .await;
        harness.provider.release_issue.notify_one();
        let error = worker.await.unwrap().unwrap_err();
        assert_eq!(
            error,
            SponsorshipError::Store(SponsorshipStoreError::LaunchAuthorityChanged)
        );
        assert!(harness.store.snapshot().await.grants.is_empty());
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
        let state = harness.store.state.lock().await;
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::Cleaned);
        assert!(!intent.capacity_held);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn usage_reservation_is_atomic_and_only_metering_can_settle_it() {
        let limits = Limits {
            per_launch_quota: SponsoredQuota {
                requests: 2,
                input_tokens: 200,
                output_tokens: 200,
            },
            aggregate_quota: SponsoredQuota {
                requests: 2,
                input_tokens: 200,
                output_tokens: 200,
            },
            per_launch_budget_micros: 200,
            aggregate_budget_micros: 200,
            max_requests_per_minute: 100,
            max_concurrent_launches: 1,
        };
        let harness = Arc::new(harness(limits).await);
        let grant = Arc::new(activate_for_usage(&harness).await);
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };
        let mut tasks = Vec::new();
        for index in 0..16 {
            let harness = harness.clone();
            let grant = grant.clone();
            tasks.push(tokio::spawn(async move {
                harness
                    .service
                    .reserve_usage(
                        &harness.contributor,
                        usage_command(&grant, format!("usage-request-{index}"), quota),
                    )
                    .await
            }));
        }
        let mut accepted = Vec::new();
        for task in tasks {
            if let Ok(outcome) = task.await.unwrap() {
                accepted.push(outcome.reservation);
            }
        }
        assert_eq!(accepted.len(), 2);
        let first = accepted.pop().unwrap();
        let unauthorized = harness
            .service
            .reconcile_usage(
                &harness.contributor,
                ReportSponsoredUsage {
                    reservation_id: first.id.clone(),
                    request_fingerprint: first.request_fingerprint.clone(),
                    actual_quota: Some(quota),
                    outcome: SponsoredUsageState::Settled,
                },
            )
            .await
            .unwrap_err();
        assert_eq!(unauthorized, SponsorshipError::Forbidden);
        let settled = harness
            .service
            .reconcile_usage(
                &harness.meter,
                ReportSponsoredUsage {
                    reservation_id: first.id,
                    request_fingerprint: first.request_fingerprint,
                    actual_quota: Some(quota),
                    outcome: SponsoredUsageState::Settled,
                },
            )
            .await
            .unwrap();
        assert_eq!(settled.actual.unwrap().quota, quota);
        let snapshot = harness.store.snapshot().await;
        assert_eq!(snapshot.reservations.len(), 2);
        assert!(snapshot
            .audits
            .iter()
            .any(|event| event.action == SponsoredAuditAction::UsageReserved));
        assert!(snapshot
            .audits
            .iter()
            .any(|event| event.action == SponsoredAuditAction::UsageReconciled));
    }

    #[tokio::test]
    async fn usage_and_terminal_grant_timestamps_survive_a_service_clock_rewind() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };

        harness.clock.set(LATER);
        let reservation = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "clock-rewind-usage", quota),
            )
            .await
            .unwrap()
            .reservation;
        assert_eq!(reservation.created_at, LATER);

        let mut regressed_model = reservation.clone();
        regressed_model.updated_at = NOW.into();
        assert!(matches!(
            regressed_model.validate(),
            Err(TriggerValidationError { .. })
        ));

        harness.clock.set(NOW);
        let ambiguous = harness
            .service
            .reconcile_usage(
                &harness.meter,
                ReportSponsoredUsage {
                    reservation_id: reservation.id.clone(),
                    request_fingerprint: reservation.request_fingerprint.clone(),
                    actual_quota: None,
                    outcome: SponsoredUsageState::Ambiguous,
                },
            )
            .await
            .unwrap();
        assert_eq!(ambiguous.updated_at, LATER);

        let stale_audit = SponsoredAuditEvent::privacy_safe(
            &harness.audit,
            SponsoredAuditAction::UsageReconciled,
            &harness.meter.subject,
            SponsoredAuditTarget::Platform,
            Some(&reservation.id.0),
            "released",
            NOW,
        );
        assert_eq!(
            harness
                .store
                .reconcile_usage(
                    ReconcileSponsoredUsage {
                        reservation_id: reservation.id.clone(),
                        request_fingerprint: reservation.request_fingerprint.clone(),
                        actual_quota: None,
                        actual: None,
                        outcome: SponsoredUsageState::Released,
                        observed_at: NOW.into(),
                    },
                    stale_audit,
                )
                .await,
            Err(SponsorshipStoreError::Conflict)
        );

        harness.clock.set(LATER);
        harness.store.set_authoritative_now(GRANT_EXPIRY).await;
        let expired = harness
            .service
            .expire_grant(&harness.meter, &grant.id)
            .await
            .unwrap();
        assert_eq!(expired.state, SponsoredGrantState::Expired);
        assert_eq!(expired.updated_at, LATER);

        harness.clock.set(NOW);
        let settled_report = ReportSponsoredUsage {
            reservation_id: reservation.id,
            request_fingerprint: reservation.request_fingerprint,
            actual_quota: Some(quota),
            outcome: SponsoredUsageState::Settled,
        };
        let settled = harness
            .service
            .reconcile_usage(&harness.meter, settled_report.clone())
            .await
            .unwrap();
        assert_eq!(settled.updated_at, LATER);
        let (settled_grant, settled_usage) = {
            let state = harness.store.state.lock().await;
            (
                state.grants.get(&grant.id.0).unwrap().clone(),
                state
                    .policy_usage
                    .get(&harness.policy.id.0)
                    .unwrap()
                    .clone(),
            )
        };
        assert_eq!(settled_grant.state, SponsoredGrantState::Expired);
        assert_eq!(settled_grant.updated_at, LATER);
        assert_eq!(settled_usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(settled_usage.consumed, settled.actual.clone().unwrap());

        assert_eq!(
            harness
                .service
                .reconcile_usage(&harness.meter, settled_report)
                .await
                .unwrap(),
            settled
        );
        let state = harness.store.state.lock().await;
        assert_eq!(state.grants.get(&grant.id.0), Some(&settled_grant));
        assert_eq!(
            state.policy_usage.get(&harness.policy.id.0),
            Some(&settled_usage)
        );
    }

    #[tokio::test]
    async fn stable_usage_retry_returns_original_before_clock_or_repricing() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };
        let command = usage_command(&grant, "stable-usage-retry", quota);
        let created = harness
            .service
            .reserve_usage(&harness.contributor, command.clone())
            .await
            .unwrap();
        assert_eq!(created.disposition, ReservationDisposition::Created);
        assert_eq!(harness.pricing.quote_calls.load(Ordering::SeqCst), 1);

        harness
            .service
            .engage_kill_switch(&harness.admin)
            .await
            .unwrap();
        harness.clock.set("not-a-canonical-timestamp");
        *harness.pricing.quote_error.lock().await = Some(SponsoredPricingError::Unavailable);
        let replayed = harness
            .service
            .reserve_usage(&harness.contributor, command)
            .await
            .unwrap();
        assert_eq!(replayed.disposition, ReservationDisposition::Existing);
        assert_eq!(replayed.reservation, created.reservation);
        assert_eq!(harness.pricing.quote_calls.load(Ordering::SeqCst), 1);

        assert_eq!(
            harness
                .service
                .reserve_usage(
                    &harness.contributor,
                    usage_command(
                        &grant,
                        "stable-usage-retry",
                        SponsoredQuota {
                            requests: 1,
                            input_tokens: 99,
                            output_tokens: 100,
                        },
                    ),
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::Conflict)
        );
        assert_eq!(harness.pricing.quote_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn usage_rotation_replaces_five_aliases_without_changing_the_opaque_handle() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let command = usage_command(
            &grant,
            "usage-five-key-replay",
            SponsoredQuota {
                requests: 1,
                input_tokens: 10,
                output_tokens: 10,
            },
        );
        let created = harness
            .service
            .reserve_usage(&harness.contributor, command.clone())
            .await
            .unwrap();

        let five_key_operational = Arc::new(
            SponsorshipOperationalPseudonymizer::new(
                "fixture-current-4",
                vec![0x64; 32],
                vec![
                    ("fixture-current-3".into(), vec![0x63; 32]),
                    ("fixture-current-2".into(), vec![0x62; 32]),
                    ("fixture-current-1".into(), vec![0x61; 32]),
                    ("fixture-current".into(), vec![0x5a; 32]),
                ],
            )
            .unwrap(),
        );
        let five_key_candidates = five_key_operational
            .pseudonymize(
                SponsorshipOperationalDomain::UsageRequest,
                &[
                    &grant.id.0,
                    &harness.contributor.subject.0,
                    "usage-five-key-replay",
                ],
            )
            .unwrap();
        let replayed = service_with_operational(&harness, five_key_operational)
            .reserve_usage(&harness.contributor, command.clone())
            .await
            .unwrap();
        assert_eq!(replayed.disposition, ReservationDisposition::Existing);
        assert_eq!(replayed.reservation, created.reservation);
        {
            let state = harness.store.state.lock().await;
            assert_eq!(
                state
                    .reservation_aliases
                    .get(&created.reservation.id.0)
                    .unwrap(),
                &pseudonym_aliases(&five_key_candidates)
            );
            assert_eq!(
                state
                    .reservation_aliases
                    .get(&created.reservation.id.0)
                    .unwrap()
                    .len(),
                5
            );
        }

        let without_original = Arc::new(
            SponsorshipOperationalPseudonymizer::new(
                "fixture-current-5",
                vec![0x65; 32],
                vec![
                    ("fixture-current-4".into(), vec![0x64; 32]),
                    ("fixture-current-3".into(), vec![0x63; 32]),
                    ("fixture-current-2".into(), vec![0x62; 32]),
                    ("fixture-current-1".into(), vec![0x61; 32]),
                ],
            )
            .unwrap(),
        );
        let migrated_candidates = without_original
            .pseudonymize(
                SponsorshipOperationalDomain::UsageRequest,
                &[
                    &grant.id.0,
                    &harness.contributor.subject.0,
                    "usage-five-key-replay",
                ],
            )
            .unwrap();
        let migrated = service_with_operational(&harness, without_original)
            .reserve_usage(&harness.contributor, command)
            .await
            .unwrap();
        assert_eq!(migrated.reservation, created.reservation);
        let state = harness.store.state.lock().await;
        let stored_aliases = state
            .reservation_aliases
            .get(&created.reservation.id.0)
            .unwrap();
        assert_eq!(stored_aliases, &pseudonym_aliases(&migrated_candidates));
        assert!(!stored_aliases
            .iter()
            .any(|alias| alias.starts_with("sp1.fixture-current.")));
        assert_eq!(harness.pricing.quote_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn predictable_usage_ids_are_scoped_and_split_bindings_fail_closed() {
        const PREDICTABLE_REQUEST_ID: &str = "12345678";
        let harness = default_harness().await;
        let first_grant = activate_for_usage(&harness).await;
        let (second_launch, second_session) =
            prepare_additional_activation(&harness, "usage-id-scope").await;
        let second_grant = harness
            .service
            .activate_grant(
                &harness.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "usage-id-scope-grant".into(),
                    launch_id: second_launch.clone(),
                },
            )
            .await
            .unwrap();
        harness
            .store
            .update_launch_state(
                &second_launch,
                SponsoredLaunchState::Active,
                Some(second_session),
            )
            .await;
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 10,
            output_tokens: 10,
        };
        let first_command = usage_command(&first_grant, PREDICTABLE_REQUEST_ID, quota);
        let first = harness
            .service
            .reserve_usage(&harness.contributor, first_command.clone())
            .await
            .unwrap()
            .reservation;
        let second = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&second_grant, PREDICTABLE_REQUEST_ID, quota),
            )
            .await
            .unwrap()
            .reservation;
        assert_ne!(first.id, second.id);
        assert_ne!(first.id.0, PREDICTABLE_REQUEST_ID);
        assert_ne!(second.id.0, PREDICTABLE_REQUEST_ID);

        let other_caller = caller(
            SubjectId("other-usage-contributor".into()),
            [Capability::UseOwnSessions],
        );
        assert_eq!(
            harness
                .service
                .reserve_usage(
                    &other_caller,
                    usage_command(&first_grant, PREDICTABLE_REQUEST_ID, quota),
                )
                .await
                .unwrap_err(),
            SponsorshipError::GrantUnavailable
        );
        assert!(matches!(
            harness
                .service
                .reconcile_usage(
                    &harness.meter,
                    ReportSponsoredUsage {
                        reservation_id: SponsoredUsageReservationId(PREDICTABLE_REQUEST_ID.into()),
                        request_fingerprint: first.request_fingerprint.clone(),
                        actual_quota: None,
                        outcome: SponsoredUsageState::Released,
                    },
                )
                .await,
            Err(SponsorshipError::Validation(_))
        ));

        let first_aliases = {
            let state = harness.store.state.lock().await;
            let first_aliases = state.reservation_aliases.get(&first.id.0).unwrap().clone();
            let second_aliases = state.reservation_aliases.get(&second.id.0).unwrap();
            assert!(first_aliases.is_disjoint(second_aliases));
            first_aliases
        };
        {
            let mut state = harness.store.state.lock().await;
            state
                .reservation_aliases
                .get_mut(&second.id.0)
                .unwrap()
                .insert(first_aliases.into_iter().next().unwrap());
        }
        assert!(matches!(
            harness
                .service
                .reserve_usage(&harness.contributor, first_command)
                .await,
            Err(SponsorshipError::Store(SponsorshipStoreError::Malformed(_)))
        ));
    }

    #[tokio::test]
    async fn terminal_usage_replays_bypass_clock_and_pricing_but_conflicts_fail() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };
        let settled_candidate = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "terminal-settlement-replay", quota),
            )
            .await
            .unwrap()
            .reservation;
        let released_candidate = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "terminal-release-replay", quota),
            )
            .await
            .unwrap()
            .reservation;
        let settled_report = ReportSponsoredUsage {
            reservation_id: settled_candidate.id.clone(),
            request_fingerprint: settled_candidate.request_fingerprint.clone(),
            actual_quota: Some(quota),
            outcome: SponsoredUsageState::Settled,
        };
        let settled = harness
            .service
            .reconcile_usage(&harness.meter, settled_report.clone())
            .await
            .unwrap();
        let released_report = ReportSponsoredUsage {
            reservation_id: released_candidate.id,
            request_fingerprint: released_candidate.request_fingerprint,
            actual_quota: None,
            outcome: SponsoredUsageState::Released,
        };
        let released = harness
            .service
            .reconcile_usage(&harness.meter, released_report.clone())
            .await
            .unwrap();
        assert_eq!(harness.pricing.actual_calls.load(Ordering::SeqCst), 1);

        harness.clock.set("not-a-canonical-timestamp");
        *harness.pricing.actual_error.lock().await = Some(SponsoredPricingError::Unavailable);
        assert_eq!(
            harness
                .service
                .reconcile_usage(&harness.contributor, settled_report.clone())
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );
        assert_eq!(
            harness
                .service
                .reconcile_usage(&harness.meter, settled_report.clone())
                .await
                .unwrap(),
            settled
        );
        assert_eq!(
            harness
                .service
                .reconcile_usage(
                    &harness.meter,
                    ReportSponsoredUsage {
                        actual_quota: Some(SponsoredQuota {
                            requests: 1,
                            input_tokens: 99,
                            output_tokens: 100,
                        }),
                        ..settled_report
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::Conflict)
        );
        assert_eq!(
            harness
                .service
                .reconcile_usage(&harness.meter, released_report.clone())
                .await
                .unwrap(),
            released
        );
        assert_eq!(
            harness
                .service
                .reconcile_usage(
                    &harness.meter,
                    ReportSponsoredUsage {
                        actual_quota: Some(quota),
                        outcome: SponsoredUsageState::Settled,
                        ..released_report
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::Conflict)
        );
        assert_eq!(harness.pricing.actual_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn terminal_usage_direct_store_replay_rejects_mismatched_actual_quota() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };
        let reservation = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "direct-store-terminal-replay", quota),
            )
            .await
            .unwrap()
            .reservation;
        let settled = harness
            .service
            .reconcile_usage(
                &harness.meter,
                ReportSponsoredUsage {
                    reservation_id: reservation.id,
                    request_fingerprint: reservation.request_fingerprint,
                    actual_quota: Some(quota),
                    outcome: SponsoredUsageState::Settled,
                },
            )
            .await
            .unwrap();
        let (replay_audit, audit_count) = {
            let state = harness.store.state.lock().await;
            (state.audits.last().unwrap().clone(), state.audits.len())
        };

        assert_eq!(
            harness
                .store
                .reconcile_usage(
                    ReconcileSponsoredUsage {
                        reservation_id: settled.id.clone(),
                        request_fingerprint: settled.request_fingerprint.clone(),
                        actual_quota: Some(SponsoredQuota {
                            requests: 1,
                            input_tokens: 99,
                            output_tokens: 100,
                        }),
                        actual: settled.actual.clone(),
                        outcome: SponsoredUsageState::Settled,
                        observed_at: LATER.into(),
                    },
                    replay_audit,
                )
                .await,
            Err(SponsorshipStoreError::Conflict)
        );
        let state = harness.store.state.lock().await;
        assert_eq!(state.reservations.get(&settled.id.0), Some(&settled));
        assert_eq!(state.audits.len(), audit_count);
    }

    #[tokio::test]
    async fn terminal_grant_retains_unresolved_usage_holds_until_reconciliation() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };
        let first = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "terminal-hold-reserved", quota),
            )
            .await
            .unwrap()
            .reservation;
        let second = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "terminal-hold-ambiguous", quota),
            )
            .await
            .unwrap()
            .reservation;
        harness
            .service
            .reconcile_usage(
                &harness.meter,
                ReportSponsoredUsage {
                    reservation_id: second.id.clone(),
                    request_fingerprint: second.request_fingerprint.clone(),
                    actual_quota: None,
                    outcome: SponsoredUsageState::Ambiguous,
                },
            )
            .await
            .unwrap();

        harness.clock.set(GRANT_EXPIRY);
        harness.store.set_authoritative_now(GRANT_EXPIRY).await;
        let expired = harness
            .service
            .expire_grant(&harness.meter, &grant.id)
            .await
            .unwrap();
        assert_eq!(expired.state, SponsoredGrantState::Expired);
        let held = first.reserved.checked_add(&second.reserved).unwrap();
        {
            let state = harness.store.state.lock().await;
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved, held);
            assert_eq!(usage.active_launches, 0);
        }

        harness
            .service
            .reconcile_usage(
                &harness.meter,
                ReportSponsoredUsage {
                    reservation_id: first.id,
                    request_fingerprint: first.request_fingerprint,
                    actual_quota: None,
                    outcome: SponsoredUsageState::Released,
                },
            )
            .await
            .unwrap();
        {
            let state = harness.store.state.lock().await;
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved, second.reserved);
        }

        harness
            .service
            .reconcile_usage(
                &harness.meter,
                ReportSponsoredUsage {
                    reservation_id: second.id,
                    request_fingerprint: second.request_fingerprint,
                    actual_quota: Some(quota),
                    outcome: SponsoredUsageState::Settled,
                },
            )
            .await
            .unwrap();
        let state = harness.store.state.lock().await;
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(
            usage.consumed,
            SponsoredUsageSummary {
                quota,
                spend_micros: 100,
            }
        );
        assert_eq!(
            state.grants.get(&grant.id.0).unwrap().state,
            SponsoredGrantState::Expired
        );
    }

    #[tokio::test]
    async fn malformed_actual_pricing_quota_is_rejected() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let reserved_quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };
        let reservation = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "malformed-actual-price", reserved_quota),
            )
            .await
            .unwrap()
            .reservation;
        let reported_quota = SponsoredQuota {
            requests: 1,
            input_tokens: 50,
            output_tokens: 50,
        };
        *harness.pricing.actual_quota_override.lock().await = Some(SponsoredQuota {
            requests: 1,
            input_tokens: 49,
            output_tokens: 50,
        });
        assert_eq!(
            harness
                .service
                .reconcile_usage(
                    &harness.meter,
                    ReportSponsoredUsage {
                        reservation_id: reservation.id.clone(),
                        request_fingerprint: reservation.request_fingerprint,
                        actual_quota: Some(reported_quota),
                        outcome: SponsoredUsageState::Settled,
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::PricingUnsupported
        );
        assert_eq!(
            harness
                .store
                .usage_reservation(&reservation.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            SponsoredUsageState::Reserved
        );
    }

    #[tokio::test]
    async fn any_mandatory_limit_exhausts_grant_and_releases_unused_capacity() {
        let limits = Limits {
            per_launch_quota: SponsoredQuota {
                requests: 4,
                input_tokens: 100,
                output_tokens: 1_000,
            },
            aggregate_quota: SponsoredQuota {
                requests: 4,
                input_tokens: 100,
                output_tokens: 1_000,
            },
            per_launch_budget_micros: 400,
            aggregate_budget_micros: 400,
            max_requests_per_minute: 10,
            max_concurrent_launches: 1,
        };
        let harness = harness(limits).await;
        let grant = activate_for_usage(&harness).await;
        let actual_quota = SponsoredQuota {
            requests: 1,
            input_tokens: 100,
            output_tokens: 100,
        };
        let reservation = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "single-dimension-exhaustion", actual_quota),
            )
            .await
            .unwrap()
            .reservation;
        harness
            .service
            .reconcile_usage(
                &harness.meter,
                ReportSponsoredUsage {
                    reservation_id: reservation.id,
                    request_fingerprint: reservation.request_fingerprint,
                    actual_quota: Some(actual_quota),
                    outcome: SponsoredUsageState::Settled,
                },
            )
            .await
            .unwrap();
        let state = harness.store.state.lock().await;
        let exhausted = state.grants.get(&grant.id.0).unwrap();
        assert_eq!(exhausted.state, SponsoredGrantState::Exhausted);
        assert_ne!(exhausted.usage, exhausted.allocation);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn successful_credential_rotation_preserves_existing_grant_spend() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let issuing_generation = grant.credential_generation;
        let rotated = harness
            .service
            .rotate_credential(
                &harness.sponsor,
                RotateSponsoredCredential {
                    credential_id: harness.policy.credential_id.clone(),
                    stable_request_id: "rotate-with-live-grant".into(),
                    replacement: ProviderCredentialSecret::new("replacement-credential").unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(rotated.state, SponsoredCredentialState::Active);
        let current_generation = harness
            .store
            .credential(&harness.policy.credential_id)
            .await
            .unwrap()
            .unwrap()
            .record
            .generation;
        assert_ne!(current_generation, issuing_generation);

        let reservation = harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(
                    &grant,
                    "usage-after-credential-rotation",
                    SponsoredQuota {
                        requests: 1,
                        input_tokens: 100,
                        output_tokens: 100,
                    },
                ),
            )
            .await
            .unwrap();
        assert_eq!(reservation.disposition, ReservationDisposition::Created);
        assert_eq!(harness.provider.rotate_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn close_and_expiry_release_unused_policy_capacity() {
        let close_harness = default_harness().await;
        let close_grant = activate_for_usage(&close_harness).await;
        close_harness
            .store
            .update_launch_state(
                &close_harness.launch_id,
                SponsoredLaunchState::Closed,
                Some(close_harness.session_id.clone()),
            )
            .await;
        let closed = close_harness
            .service
            .close_grant(
                &close_harness.contributor,
                &close_grant.id,
                "close-grant-request",
            )
            .await
            .unwrap();
        assert_eq!(closed.state, SponsoredGrantState::Closed);
        let close_state = close_harness.store.state.lock().await;
        let usage = close_state
            .policy_usage
            .get(&close_harness.policy.id.0)
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        drop(close_state);

        let expiry_harness = default_harness().await;
        prepare_activation(&expiry_harness).await;
        let expiry_grant = activate(&expiry_harness, "expiry-grant-request").await;
        expiry_harness.clock.set(GRANT_EXPIRY);
        expiry_harness
            .store
            .set_authoritative_now(GRANT_EXPIRY)
            .await;
        let expired = expiry_harness
            .service
            .expire_grant(&expiry_harness.meter, &expiry_grant.id)
            .await
            .unwrap();
        assert_eq!(expired.state, SponsoredGrantState::Expired);
        let expiry_state = expiry_harness.store.state.lock().await;
        let usage = expiry_state
            .policy_usage
            .get(&expiry_harness.policy.id.0)
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn authorization_canonical_ids_and_public_views_fail_closed() {
        let harness = default_harness().await;
        let outsider = caller(
            SubjectId("outsider".into()),
            Authorization::none().capabilities(),
        );
        assert_eq!(
            harness
                .service
                .terms(&outsider, &harness.launch_id)
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );
        assert_eq!(
            harness
                .service
                .suspend_policy(&harness.contributor, &harness.policy.id)
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );
        assert!(SponsoredGrantId::parse("550E8400-E29B-41D4-A716-446655440000").is_err());
        assert!(SponsoredGrantId::parse("550e8400e29b41d4a716446655440000").is_err());

        let snapshot = harness.store.snapshot().await;
        let credential = snapshot.credentials.first().unwrap();
        let wire = serde_json::to_string(&SponsoredCredentialView::from(credential)).unwrap();
        let diagnostics = format!("{credential:?}");
        for forbidden in [SECRET, "internal-policy-handle", "external-credential-id"] {
            assert!(!wire.contains(forbidden));
            assert!(!diagnostics.contains(forbidden));
        }
        assert!(
            format!("{:?}", ProviderCredentialSecret::new(SECRET).unwrap()).contains("redacted")
        );
        assert!(format!(
            "{:?}",
            SensitiveProviderValue::new("raw-provider-authority").unwrap()
        )
        .contains("redacted"));
        assert!(!format!("{:?}", harness.admin).contains(SECRET));
        assert!(!format!("{:?}", harness.sponsor).contains(SECRET));
    }

    #[tokio::test]
    async fn owner_safe_reads_and_publication_checks_remain_secret_free_and_recoverable() {
        let harness = default_harness().await;
        let binding = SponsoredPolicyBinding {
            tenant_id: harness.policy.tenant_id.clone(),
            policy_id: harness.policy.id.clone(),
        };
        harness
            .service
            .verify_publisher_policy_binding(&harness.sponsor.subject, &binding)
            .await
            .expect("the sponsoring publisher controls the exact active policy");
        assert_eq!(
            harness
                .service
                .verify_publisher_policy_binding(&SubjectId("foreign-publisher".into()), &binding)
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );

        let credential = harness
            .service
            .credential_view(&harness.sponsor, &harness.policy.credential_id)
            .await
            .unwrap();
        assert_eq!(credential.id, harness.policy.credential_id);
        let policy = harness
            .service
            .policy_view(&harness.sponsor, &harness.policy.id)
            .await
            .unwrap();
        assert_eq!(policy.id, harness.policy.id);
        assert_eq!(policy.usage, SponsorshipPolicyUsage::default());
        assert_eq!(
            harness
                .service
                .credential_view(&harness.contributor, &harness.policy.credential_id)
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );
        let review = harness
            .service
            .terms_review(&harness.contributor, &harness.launch_id)
            .await
            .unwrap();
        assert_eq!(review.terms, harness.terms);
        assert_eq!(
            review.launch_snapshot_digest,
            digest(b"fixture-launch-snapshot")
        );

        prepare_activation(&harness).await;
        let grant = activate(&harness, "grant-owner-safe-view").await;
        let contributor_view = harness
            .service
            .grant_view(&harness.contributor, &grant.id)
            .await
            .unwrap();
        let sponsor_view = harness
            .service
            .grant_for_launch_view(&harness.sponsor, &harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(contributor_view, sponsor_view);
        assert_eq!(contributor_view.id, grant.id);

        let audit = harness
            .service
            .actor_audit_events(&harness.sponsor, MAX_SPONSORED_AUDIT_PAGE)
            .await
            .unwrap();
        assert!(!audit.is_empty());
        assert_eq!(
            harness
                .service
                .actor_audit_events(&harness.admin, 1)
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );

        let fence = harness
            .service
            .engage_kill_switch(&harness.admin)
            .await
            .unwrap();
        assert!(fence.kill_switch_engaged);
        assert_eq!(
            harness
                .service
                .engage_kill_switch(&harness.admin)
                .await
                .expect("a bodyless kill-switch retry is idempotent"),
            fence
        );
        assert_eq!(
            harness
                .service
                .verify_publisher_policy_binding(&harness.sponsor.subject, &binding)
                .await
                .unwrap_err(),
            SponsorshipError::KillSwitch
        );
        harness
            .service
            .credential_view(&harness.sponsor, &harness.policy.credential_id)
            .await
            .expect("credential recovery metadata remains readable after the kill switch");
        harness
            .service
            .policy_view(&harness.sponsor, &harness.policy.id)
            .await
            .expect("policy recovery metadata remains readable after the kill switch");
        harness
            .service
            .grant_view(&harness.contributor, &grant.id)
            .await
            .expect("Launch recovery metadata remains readable after the kill switch");
    }

    #[tokio::test]
    async fn metadata_reads_reject_malformed_store_records_before_projection() {
        fn assert_malformed<T: std::fmt::Debug>(result: Result<T, SponsorshipError>) {
            assert!(
                matches!(
                    result,
                    Err(SponsorshipError::Store(SponsorshipStoreError::Malformed(_)))
                ),
                "malformed durable metadata must fail closed: {result:?}"
            );
        }

        let harness = default_harness().await;
        prepare_activation(&harness).await;
        let grant = activate(&harness, "malformed-metadata-projection").await;
        let binding = SponsoredPolicyBinding {
            tenant_id: harness.policy.tenant_id.clone(),
            policy_id: harness.policy.id.clone(),
        };

        let original_credential = {
            let mut state = harness.store.state.lock().await;
            let (credential, _) = state
                .credentials
                .get_mut(&harness.policy.credential_id.0)
                .unwrap();
            let original = credential.clone();
            credential.generation = 0;
            original
        };
        assert_malformed(
            harness
                .service
                .verify_publisher_policy_binding(&harness.sponsor.subject, &binding)
                .await,
        );
        assert_malformed(
            harness
                .service
                .credential_view(&harness.sponsor, &harness.policy.credential_id)
                .await,
        );
        assert_malformed(
            harness
                .service
                .credential_views(&harness.sponsor, &PageRequest::default())
                .await,
        );
        assert_malformed(
            harness
                .service
                .create_policy(
                    &harness.sponsor,
                    CreateSponsorshipPolicy {
                        tenant_id: "malformed-credential-policy".into(),
                        owner: owner(),
                        credential_id: harness.policy.credential_id.clone(),
                        provider_policy: SponsoredProviderPolicy {
                            provider: PROVIDER.into(),
                            provider_policy_key: POLICY_KEY.into(),
                            allowed_models: BTreeSet::from([MODEL.into()]),
                            allowed_resources: BTreeSet::from([RESOURCE.into()]),
                        },
                        per_launch_quota: Limits::default().per_launch_quota,
                        aggregate_quota: Limits::default().aggregate_quota,
                        per_launch_budget_micros: Limits::default().per_launch_budget_micros,
                        aggregate_budget_micros: Limits::default().aggregate_budget_micros,
                        max_requests_per_minute: Limits::default().max_requests_per_minute,
                        max_concurrent_launches: Limits::default().max_concurrent_launches,
                        expires_at: POLICY_EXPIRY.into(),
                    },
                )
                .await,
        );
        {
            let mut state = harness.store.state.lock().await;
            state
                .credentials
                .get_mut(&harness.policy.credential_id.0)
                .unwrap()
                .0 = original_credential;
        }

        let original_policy = {
            let mut state = harness.store.state.lock().await;
            let policy = state.policies.get_mut(&harness.policy.id.0).unwrap();
            let original = policy.clone();
            policy.generation = 0;
            original
        };
        assert_malformed(
            harness
                .service
                .policy_view(&harness.sponsor, &harness.policy.id)
                .await,
        );
        assert_malformed(
            harness
                .service
                .policy_views(&harness.sponsor, &PageRequest::default())
                .await,
        );
        {
            harness
                .store
                .state
                .lock()
                .await
                .policies
                .insert(harness.policy.id.0.clone(), original_policy);
        }

        let original_sponsor_subject = {
            let mut state = harness.store.state.lock().await;
            let grant = state.grants.get_mut(&grant.id.0).unwrap();
            let original = grant.sponsor_subject.clone();
            grant.sponsor_subject = SubjectId("different-sponsor-subject".into());
            grant.refresh_capability().unwrap();
            grant
                .validate()
                .expect("the corrupted grant remains valid in isolation");
            original
        };
        assert_malformed(
            harness
                .service
                .grant_view(&harness.contributor, &grant.id)
                .await,
        );
        assert_malformed(
            harness
                .service
                .grant_for_launch_view(&harness.sponsor, &grant.launch_id)
                .await,
        );
        assert_malformed(
            harness
                .service
                .contributor_grant_views(&harness.contributor, &PageRequest::default())
                .await,
        );
        assert_malformed(
            harness
                .service
                .sponsor_grant_views(&harness.sponsor, &PageRequest::default())
                .await,
        );
        {
            let mut state = harness.store.state.lock().await;
            let grant = state.grants.get_mut(&grant.id.0).unwrap();
            grant.sponsor_subject = original_sponsor_subject;
            grant.refresh_capability().unwrap();
            grant.validate().unwrap();
        }

        {
            let mut state = harness.store.state.lock().await;
            state.grants.get_mut(&grant.id.0).unwrap().generation = 0;
        }
        assert_malformed(
            harness
                .service
                .grant_view(&harness.contributor, &grant.id)
                .await,
        );
        assert_malformed(
            harness
                .service
                .grant_for_launch_view(&harness.sponsor, &grant.launch_id)
                .await,
        );
        assert_malformed(
            harness
                .service
                .contributor_grant_views(&harness.contributor, &PageRequest::default())
                .await,
        );
        assert_malformed(
            harness
                .service
                .sponsor_grant_views(&harness.sponsor, &PageRequest::default())
                .await,
        );
    }

    #[tokio::test]
    async fn registration_is_unique_idempotent_and_audit_identifiers_are_keyed() {
        let harness = default_harness().await;
        harness.clock.set(LATER);
        let retry = harness
            .service
            .register_credential(
                &harness.sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "register-credential-a".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new(SECRET).unwrap(),
                },
            )
            .await
            .unwrap();
        let alias = harness
            .service
            .register_credential(
                &harness.sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "register-credential-alias".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new(SECRET).unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(retry.id, harness.policy.credential_id);
        assert_eq!(alias.id, retry.id);

        let state = harness.store.state.lock().await;
        assert_eq!(state.credentials.len(), 1);
        assert_eq!(state.credential_registrations.len(), 2);
        drop(state);

        let actor_digest = harness.audit.actor_digest(&harness.sponsor.subject);
        assert_ne!(actor_digest, harness.sponsor.subject.0);
        assert_ne!(
            actor_digest,
            sensitive_test_digest("cma-sponsored-audit-actor-v1", &harness.sponsor.subject.0)
        );
        let other_key = SponsorshipAuditPseudonymizer::new(vec![0x5a; 32]).unwrap();
        assert_ne!(
            actor_digest,
            other_key.actor_digest(&harness.sponsor.subject)
        );
        let events = harness
            .store
            .audit_events(&actor_digest, MAX_SPONSORED_AUDIT_PAGE)
            .await
            .unwrap();
        assert!(!events.is_empty());
        let wire = serde_json::to_string(&events).unwrap();
        assert!(!wire.contains(&harness.sponsor.subject.0));
        assert!(!wire.contains("register-credential-a"));
        assert!(events.iter().all(|event| {
            event.actor_digest == actor_digest
                && event
                    .stable_request_id_digest
                    .as_ref()
                    .is_none_or(|digest| digest.len() == 64)
        }));
        assert!(format!("{:?}", harness.audit).contains("redacted"));
    }

    #[tokio::test]
    async fn registration_replay_and_conflicts_precede_provider_access_during_outage() {
        let harness = default_harness().await;
        let baseline = {
            let state = harness.store.state.lock().await;
            (
                state.credentials.len(),
                state.credential_registrations.len(),
                state.audits.len(),
                harness.provider.ownership_calls.load(Ordering::SeqCst),
            )
        };
        assert_eq!(baseline.3, 1);
        *harness.provider.ownership_mode.lock().await = ProviderMode::Unavailable;

        let replay = harness
            .service
            .register_credential(
                &harness.sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "register-credential-a".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new(SECRET).unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(replay.id, harness.policy.credential_id);
        assert_eq!(
            harness.provider.ownership_calls.load(Ordering::SeqCst),
            baseline.3,
            "exact replay must not reach ownership validation"
        );

        let changed_commands = [
            (
                SponsorBillingOwner::Developer {
                    subject: subject(),
                    display_name: "Changed Sponsor Name".into(),
                },
                PROVIDER,
                POLICY_KEY,
                SECRET,
            ),
            (owner(), "provider-b", POLICY_KEY, SECRET),
            (owner(), PROVIDER, ALTERNATE_POLICY_KEY, SECRET),
            (
                owner(),
                PROVIDER,
                POLICY_KEY,
                "predictable-replacement-secret",
            ),
        ];
        for (changed_owner, provider, policy_key, secret) in changed_commands {
            let error = harness
                .service
                .register_credential(
                    &harness.sponsor,
                    RegisterSponsoredCredential {
                        stable_request_id: "register-credential-a".into(),
                        owner: changed_owner,
                        provider: provider.into(),
                        provider_policy_key: policy_key.into(),
                        credential: ProviderCredentialSecret::new(secret).unwrap(),
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(
                error,
                SponsorshipError::Store(SponsorshipStoreError::Conflict)
            );
            assert_eq!(
                harness.provider.ownership_calls.load(Ordering::SeqCst),
                baseline.3,
                "changed bound command must conflict before provider access"
            );
        }

        let miss = harness
            .service
            .register_credential(
                &harness.sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "unbound-registration-during-outage".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new(SECRET).unwrap(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(miss, SponsorshipError::ProviderUnavailable);
        assert_eq!(
            harness.provider.ownership_calls.load(Ordering::SeqCst),
            baseline.3 + 1
        );
        let state = harness.store.state.lock().await;
        assert_eq!(state.credentials.len(), baseline.0);
        assert_eq!(state.credential_registrations.len(), baseline.1);
        assert_eq!(state.audits.len(), baseline.2);
    }

    #[tokio::test]
    async fn credential_rotation_and_revocations_reconcile_lost_responses() {
        let rotation = default_harness().await;
        *rotation.provider.rotate_mode.lock().await = ProviderMode::LostResponse;
        let rotate = || RotateSponsoredCredential {
            credential_id: rotation.policy.credential_id.clone(),
            stable_request_id: "rotate-credential-request".into(),
            replacement: ProviderCredentialSecret::new("replacement-credential").unwrap(),
        };
        assert_eq!(
            rotation
                .service
                .rotate_credential(&rotation.sponsor, rotate())
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        {
            let state = rotation.store.state.lock().await;
            let (record, current_secret) = state
                .credentials
                .get(&rotation.policy.credential_id.0)
                .unwrap();
            let operation = record.pending_operation.as_ref().unwrap();
            assert_eq!(record.state, SponsoredCredentialState::RotationPending);
            assert_eq!(
                record.material_state,
                SponsoredCredentialMaterialState::Retained
            );
            assert_eq!(current_secret.as_deref(), Some(SECRET));
            assert_eq!(
                state
                    .pending_credential_secrets
                    .get(&rotation.policy.credential_id.0)
                    .map(String::as_str),
                Some("replacement-credential")
            );
            assert_eq!(operation.kind, ProviderOperationKind::RotateCredential);
            assert_eq!(operation.phase, ProviderOperationPhase::Ambiguous);
            assert_ne!(operation.provider_request_id, "rotate-credential-request");
        }
        let restarted_rotation = restarted_service(&rotation);
        let rotated = restarted_rotation
            .rotate_credential(&rotation.sponsor, rotate())
            .await
            .unwrap();
        assert_eq!(rotated.state, SponsoredCredentialState::Active);
        assert_eq!(rotation.provider.rotate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(rotation.provider.reconcile_calls.load(Ordering::SeqCst), 1);
        let replayed = rotation
            .service
            .rotate_credential(&rotation.sponsor, rotate())
            .await
            .unwrap();
        assert_eq!(replayed, rotated);
        let alias_replay = rotation
            .service
            .rotate_credential(
                &rotation.sponsor,
                RotateSponsoredCredential {
                    credential_id: rotation.policy.credential_id.clone(),
                    stable_request_id: "rotate-credential-alias".into(),
                    replacement: ProviderCredentialSecret::new("replacement-credential").unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(alias_replay, rotated);
        assert_eq!(rotation.provider.rotate_calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            rotation
                .service
                .rotate_credential(
                    &rotation.sponsor,
                    RotateSponsoredCredential {
                        credential_id: rotation.policy.credential_id.clone(),
                        stable_request_id: "rotate-credential-request".into(),
                        replacement: ProviderCredentialSecret::new("different-replacement")
                            .unwrap(),
                    },
                )
                .await,
            Err(SponsorshipError::Store(SponsorshipStoreError::Conflict))
        ));
        let (_, secret) = rotation
            .store
            .credential(&rotation.policy.credential_id)
            .await
            .unwrap()
            .unwrap()
            .into_required_secret()
            .unwrap();
        assert_eq!(secret.expose(), "replacement-credential");
        assert!(!rotation
            .store
            .state
            .lock()
            .await
            .pending_credential_secrets
            .contains_key(&rotation.policy.credential_id.0));

        let credential_revocation = default_harness().await;
        *credential_revocation.provider.revoke_mode.lock().await = ProviderMode::LostResponse;
        assert_eq!(
            credential_revocation
                .service
                .revoke_credential(
                    &credential_revocation.sponsor,
                    &credential_revocation.policy.credential_id,
                    "revoke-credential-request",
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let revoked = credential_revocation
            .service
            .revoke_credential(
                &credential_revocation.sponsor,
                &credential_revocation.policy.credential_id,
                "revoke-credential-request",
            )
            .await
            .unwrap();
        assert_eq!(revoked.state, SponsoredCredentialState::Revoked);
        assert_eq!(
            credential_revocation
                .provider
                .revoke_calls
                .load(Ordering::SeqCst),
            1
        );

        let grant_revocation = default_harness().await;
        prepare_activation(&grant_revocation).await;
        let grant = activate(&grant_revocation, "grant-before-revocation").await;
        *grant_revocation.provider.revoke_mode.lock().await = ProviderMode::LostResponse;
        assert_eq!(
            grant_revocation
                .service
                .revoke_grant(&grant_revocation.sponsor, &grant.id, "revoke-grant-request",)
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        {
            let state = grant_revocation.store.state.lock().await;
            let held = state
                .policy_usage
                .get(&grant_revocation.policy.id.0)
                .unwrap();
            assert_eq!(held.reserved, grant.allocation);
            assert_eq!(held.active_launches, 1);
        }
        let revoked = grant_revocation
            .service
            .revoke_grant(&grant_revocation.sponsor, &grant.id, "revoke-grant-request")
            .await
            .unwrap();
        assert_eq!(revoked.state, SponsoredGrantState::Revoked);
        let state = grant_revocation.store.state.lock().await;
        let released = state
            .policy_usage
            .get(&grant_revocation.policy.id.0)
            .unwrap();
        assert_eq!(released.reserved, SponsoredUsageSummary::default());
        assert_eq!(released.active_launches, 0);
    }

    #[tokio::test]
    async fn revocation_recovery_preserves_origin_identity_and_retires_material() {
        let harness = default_harness().await;
        let original_request_id = "revocation-recovery-r1";
        let recovery_request_id = "revocation-recovery-r2";
        let operation_identity = |request_id: &str| {
            harness
                .operational
                .pseudonymize(
                    SponsorshipOperationalDomain::CredentialOperationRequest,
                    &[
                        &harness.policy.credential_id.0,
                        "revoke",
                        &harness.sponsor.subject.0,
                        request_id,
                    ],
                )
                .unwrap()
                .current()
                .as_str()
                .to_string()
        };
        let original_request_digest = operation_identity(original_request_id);
        let recovery_request_digest = operation_identity(recovery_request_id);
        assert!(original_request_digest.starts_with("sp1.fixture-current."));
        assert_ne!(original_request_digest, recovery_request_digest);
        *harness.provider.revoke_mode.lock().await = ProviderMode::LostResponse;
        assert_eq!(
            harness
                .service
                .revoke_credential(
                    &harness.sponsor,
                    &harness.policy.credential_id,
                    original_request_id,
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let provider_request_id = {
            let state = harness.store.state.lock().await;
            let (record, current_secret) = state
                .credentials
                .get(&harness.policy.credential_id.0)
                .unwrap();
            assert_eq!(record.state, SponsoredCredentialState::RevocationPending);
            assert_eq!(
                record.material_state,
                SponsoredCredentialMaterialState::Retained
            );
            assert_eq!(current_secret.as_deref(), Some(SECRET));
            assert!(record.last_completed_operation.is_none());
            let operation = record.pending_operation.as_ref().unwrap();
            assert_eq!(operation.kind, ProviderOperationKind::RevokeCredential);
            assert_eq!(operation.phase, ProviderOperationPhase::Ambiguous);
            assert_eq!(operation.client_request_id_digest, original_request_digest);
            assert!(state.credential_operations.is_empty());
            operation.provider_request_id.clone()
        };
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.reconcile_calls.load(Ordering::SeqCst), 0);

        let restarted = restarted_service(&harness);
        let revoked = restarted
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                recovery_request_id,
            )
            .await
            .unwrap();
        assert_eq!(revoked.state, SponsoredCredentialState::Revoked);
        {
            let state = harness.store.state.lock().await;
            let (record, current_secret) = state
                .credentials
                .get(&harness.policy.credential_id.0)
                .unwrap();
            record.validate().unwrap();
            assert_eq!(record.state, SponsoredCredentialState::Revoked);
            assert_eq!(
                record.material_state,
                SponsoredCredentialMaterialState::Retired
            );
            assert!(current_secret.is_none());
            assert!(record.pending_operation.is_none());
            assert!(record.superseded_operation.is_none());
            let completed = record.last_completed_operation.as_ref().unwrap();
            assert_eq!(completed.kind, ProviderOperationKind::RevokeCredential);
            assert_eq!(
                completed.outcome,
                CompletedProviderOperationOutcome::Confirmed
            );
            assert_eq!(completed.provider_request_id, provider_request_id);
            assert_eq!(completed.client_request_id_digest, original_request_digest);
            assert_eq!(state.credential_operations.len(), 1);
            assert_eq!(
                state.credential_operations.get(&(
                    harness.policy.credential_id.0.clone(),
                    ProviderOperationKind::RevokeCredential,
                    original_request_digest.clone(),
                )),
                Some(completed)
            );
            assert!(!state.credential_operations.contains_key(&(
                harness.policy.credential_id.0.clone(),
                ProviderOperationKind::RevokeCredential,
                recovery_request_digest,
            )));
            assert!(!state
                .pending_credential_secrets
                .contains_key(&harness.policy.credential_id.0));
        }
        assert!(harness
            .provider
            .revocations
            .lock()
            .await
            .contains(&provider_request_id));
        assert!(harness
            .provider
            .revoked_lineages
            .lock()
            .await
            .contains(&harness.policy.credential_id.0));
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.reconcile_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            restarted
                .rotate_credential(
                    &harness.sponsor,
                    RotateSponsoredCredential {
                        credential_id: harness.policy.credential_id.clone(),
                        stable_request_id: "rotation-after-retirement".into(),
                        replacement: ProviderCredentialSecret::new("replacement-after-retirement")
                            .unwrap(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::CredentialUnavailable
        );
        assert_eq!(harness.provider.rotate_calls.load(Ordering::SeqCst), 0);
        assert!(!harness
            .store
            .state
            .lock()
            .await
            .pending_credential_secrets
            .contains_key(&harness.policy.credential_id.0));

        for request_id in [original_request_id, recovery_request_id] {
            let replayed = restarted_service(&harness)
                .revoke_credential(&harness.sponsor, &harness.policy.credential_id, request_id)
                .await
                .unwrap();
            assert_eq!(replayed, revoked);
        }
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.reconcile_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rejected_credential_revocation_keeps_material_for_owner_recovery() {
        let harness = default_harness().await;
        *harness.provider.revoke_mode.lock().await = ProviderMode::Rejected;
        let original_request_id = "rejected-credential-revocation";
        let recovery_request_id = "rejected-revocation-recovery";
        assert_eq!(
            harness
                .service
                .revoke_credential(
                    &harness.sponsor,
                    &harness.policy.credential_id,
                    original_request_id,
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderRejected
        );
        let after_rejection = harness
            .store
            .credential(&harness.policy.credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            after_rejection
                .record
                .pending_operation
                .as_ref()
                .unwrap()
                .client_request_id_digest,
            harness
                .operational
                .pseudonymize(
                    SponsorshipOperationalDomain::CredentialOperationRequest,
                    &[
                        &harness.policy.credential_id.0,
                        "revoke",
                        &harness.sponsor.subject.0,
                        original_request_id,
                    ],
                )
                .unwrap()
                .current()
                .as_str()
        );
        assert_eq!(
            harness
                .service
                .revoke_credential(
                    &harness.sponsor,
                    &harness.policy.credential_id,
                    recovery_request_id,
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderRejected
        );
        let stored = harness
            .store
            .credential(&harness.policy.credential_id)
            .await
            .unwrap()
            .unwrap();
        stored.validate().unwrap();
        assert_eq!(
            stored.record.material_state,
            SponsoredCredentialMaterialState::Retained
        );
        assert_eq!(stored.record, after_rejection.record);
        assert_eq!(
            stored.record.pending_operation.as_ref().unwrap().phase,
            ProviderOperationPhase::Rejected
        );
        assert_eq!(stored.secret.unwrap().expose(), SECRET);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retired_credential_proof_closes_a_dependent_grant_without_secret_reuse() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let revoked_credential = harness
            .service
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                "retire-credential-with-active-grant",
            )
            .await
            .unwrap();
        assert_eq!(revoked_credential.state, SponsoredCredentialState::Revoked);
        assert!(harness
            .store
            .credential(&harness.policy.credential_id)
            .await
            .unwrap()
            .unwrap()
            .secret
            .is_none());
        harness
            .store
            .state
            .lock()
            .await
            .grant_receipts
            .remove(&grant.id.0);

        let revoked_grant = harness
            .service
            .revoke_grant(
                &harness.sponsor,
                &grant.id,
                "revoke-grant-after-credential-retirement",
            )
            .await
            .unwrap();
        assert_eq!(revoked_grant.state, SponsoredGrantState::Revoked);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
        let state = harness.store.state.lock().await;
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn emergency_revocation_supersedes_ambiguous_rotation_and_revokes_both_candidates() {
        let harness = default_harness().await;
        *harness.provider.rotate_mode.lock().await = ProviderMode::Ambiguous;
        assert_eq!(
            harness
                .service
                .rotate_credential(
                    &harness.sponsor,
                    RotateSponsoredCredential {
                        credential_id: harness.policy.credential_id.clone(),
                        stable_request_id: "ambiguous-rotation-before-revoke".into(),
                        replacement: ProviderCredentialSecret::new("replacement-credential")
                            .unwrap(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let rotation_operation_id = {
            let state = harness.store.state.lock().await;
            state
                .credentials
                .get(&harness.policy.credential_id.0)
                .unwrap()
                .0
                .pending_operation
                .as_ref()
                .unwrap()
                .provider_request_id
                .clone()
        };
        *harness.provider.revoke_mode.lock().await = ProviderMode::LostResponse;
        assert_eq!(
            harness
                .service
                .revoke_credential(
                    &harness.sponsor,
                    &harness.policy.credential_id,
                    "emergency-revoke-during-rotation",
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        {
            let state = harness.store.state.lock().await;
            let (record, current_secret) = state
                .credentials
                .get(&harness.policy.credential_id.0)
                .unwrap();
            assert_eq!(record.state, SponsoredCredentialState::RevocationPending);
            assert_eq!(
                record.material_state,
                SponsoredCredentialMaterialState::Retained
            );
            assert_eq!(current_secret.as_deref(), Some(SECRET));
            assert_eq!(
                record
                    .superseded_operation
                    .as_ref()
                    .unwrap()
                    .provider_request_id,
                rotation_operation_id
            );
            assert_eq!(
                record.pending_operation.as_ref().unwrap().kind,
                ProviderOperationKind::RevokeCredential
            );
            assert_ne!(
                record
                    .pending_operation
                    .as_ref()
                    .unwrap()
                    .provider_request_id,
                rotation_operation_id
            );
            assert!(state
                .pending_credential_secrets
                .contains_key(&harness.policy.credential_id.0));
        }
        let revoked = harness
            .service
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                "emergency-revoke-during-rotation",
            )
            .await
            .unwrap();
        assert_eq!(revoked.state, SponsoredCredentialState::Revoked);
        let state = harness.store.state.lock().await;
        let (record, current_secret) = state
            .credentials
            .get(&harness.policy.credential_id.0)
            .unwrap();
        assert_eq!(
            record.material_state,
            SponsoredCredentialMaterialState::Retired
        );
        assert!(current_secret.is_none());
        assert!(record.pending_operation.is_none());
        assert!(record.superseded_operation.is_none());
        assert!(!state
            .pending_credential_secrets
            .contains_key(&harness.policy.credential_id.0));
        assert_eq!(harness.provider.rotate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn emergency_revocation_wins_a_live_rotation_finalization_race() {
        let harness = Arc::new(default_harness().await);
        harness.provider.block_rotate.store(true, Ordering::SeqCst);
        harness
            .provider
            .block_credential_revoke
            .store(true, Ordering::SeqCst);

        let rotation = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .rotate_credential(
                        &harness.sponsor,
                        RotateSponsoredCredential {
                            credential_id: harness.policy.credential_id.clone(),
                            stable_request_id: "live-rotation-before-revoke".into(),
                            replacement: ProviderCredentialSecret::new("replacement-credential")
                                .unwrap(),
                        },
                    )
                    .await
            })
        };
        harness.provider.rotate_entered.notified().await;

        let revocation = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .revoke_credential(
                        &harness.sponsor,
                        &harness.policy.credential_id,
                        "emergency-revoke-live-rotation",
                    )
                    .await
            })
        };
        harness.provider.credential_revoke_entered.notified().await;

        {
            let state = harness.store.state.lock().await;
            let (record, current_secret) = state
                .credentials
                .get(&harness.policy.credential_id.0)
                .unwrap();
            assert_eq!(record.state, SponsoredCredentialState::RevocationPending);
            assert_eq!(
                record.material_state,
                SponsoredCredentialMaterialState::Retained
            );
            assert_eq!(current_secret.as_deref(), Some(SECRET));
            assert_eq!(
                record.pending_operation.as_ref().unwrap().phase,
                ProviderOperationPhase::Attempting
            );
            assert_eq!(
                record.superseded_operation.as_ref().unwrap().kind,
                ProviderOperationKind::RotateCredential
            );
            assert!(state
                .pending_credential_secrets
                .contains_key(&harness.policy.credential_id.0));
        }

        harness.provider.release_rotate.notify_one();
        assert_eq!(
            rotation.await.unwrap().unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );

        harness.provider.release_credential_revoke.notify_one();
        let revoked = revocation.await.unwrap().unwrap();
        assert_eq!(revoked.state, SponsoredCredentialState::Revoked);
        let state = harness.store.state.lock().await;
        let (record, current_secret) = state
            .credentials
            .get(&harness.policy.credential_id.0)
            .unwrap();
        assert_eq!(record.state, SponsoredCredentialState::Revoked);
        assert_eq!(
            record.material_state,
            SponsoredCredentialMaterialState::Retired
        );
        assert!(current_secret.is_none());
        assert!(record.pending_operation.is_none());
        assert!(record.superseded_operation.is_none());
        assert!(!state
            .pending_credential_secrets
            .contains_key(&harness.policy.credential_id.0));
        drop(state);
        assert!(harness.provider.rotations.lock().await.is_empty());
        assert_eq!(harness.provider.rotate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retired_lineage_rejects_a_rotation_that_finishes_after_revocation() {
        let harness = Arc::new(default_harness().await);
        harness.provider.block_rotate.store(true, Ordering::SeqCst);
        let rotation = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .rotate_credential(
                        &harness.sponsor,
                        RotateSponsoredCredential {
                            credential_id: harness.policy.credential_id.clone(),
                            stable_request_id: "rotation-delayed-past-revocation".into(),
                            replacement: ProviderCredentialSecret::new("replacement-credential")
                                .unwrap(),
                        },
                    )
                    .await
            })
        };
        harness.provider.rotate_entered.notified().await;

        let revoked = harness
            .service
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                "revocation-finishes-before-rotation",
            )
            .await
            .unwrap();
        assert_eq!(revoked.state, SponsoredCredentialState::Revoked);
        assert!(harness
            .provider
            .revoked_lineages
            .lock()
            .await
            .contains(&harness.policy.credential_id.0));

        harness.provider.release_rotate.notify_one();
        assert_eq!(
            rotation.await.unwrap().unwrap_err(),
            SponsorshipError::CredentialUnavailable
        );
        let stored = harness
            .store
            .credential(&harness.policy.credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.record.state, SponsoredCredentialState::Revoked);
        assert_eq!(
            stored.record.material_state,
            SponsoredCredentialMaterialState::Retired
        );
        assert!(stored.secret.is_none());
        assert!(harness.provider.rotations.lock().await.is_empty());
        assert!(!harness
            .store
            .state
            .lock()
            .await
            .pending_credential_secrets
            .contains_key(&harness.policy.credential_id.0));
    }

    #[tokio::test]
    async fn rotating_then_revoking_current_root_retires_all_earlier_lineage_work() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        let (inflight_launch, _) =
            prepare_additional_activation(&harness, "lineage-inflight").await;
        *harness.provider.issue_mode.lock().await = ProviderMode::LostResponse;
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "lineage-a-era-inflight".into(),
                        launch_id: inflight_launch.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        assert_eq!(
            harness
                .store
                .grant_activation(&inflight_launch)
                .await
                .unwrap()
                .unwrap()
                .state,
            GrantActivationState::ProviderAmbiguous
        );

        *harness.provider.issue_mode.lock().await = ProviderMode::Confirmed;
        let rotated = harness
            .service
            .rotate_credential(
                &harness.sponsor,
                RotateSponsoredCredential {
                    credential_id: harness.policy.credential_id.clone(),
                    stable_request_id: "lineage-rotate-a-to-b".into(),
                    replacement: ProviderCredentialSecret::new("replacement-credential").unwrap(),
                },
            )
            .await
            .unwrap();
        assert!(rotated.generation > grant.credential_generation);

        let revoked = harness
            .service
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                "lineage-revoke-current-b",
            )
            .await
            .unwrap();
        assert_eq!(revoked.state, SponsoredCredentialState::Revoked);
        assert!(harness
            .provider
            .revoked_lineages
            .lock()
            .await
            .contains(&harness.policy.credential_id.0));
        let operation_lineages = harness.provider.operation_lineages.lock().await;
        assert_eq!(operation_lineages.len(), 2);
        assert!(operation_lineages
            .values()
            .all(|lineage| lineage == &harness.policy.credential_id.0));
        drop(operation_lineages);

        harness
            .store
            .state
            .lock()
            .await
            .grant_receipts
            .remove(&grant.id.0);
        let provider_revocations = harness.provider.revoke_calls.load(Ordering::SeqCst);
        let closed = harness
            .service
            .revoke_grant(&harness.sponsor, &grant.id, "lineage-close-a-era-grant")
            .await
            .unwrap();
        assert_eq!(closed.state, SponsoredGrantState::Revoked);
        harness
            .service
            .reconcile_activation_cleanup(&harness.meter, &inflight_launch)
            .await
            .unwrap();
        assert_eq!(
            harness.provider.revoke_calls.load(Ordering::SeqCst),
            provider_revocations,
            "retired lineage proof must not reuse erased material"
        );
        let intent = harness
            .store
            .grant_activation(&inflight_launch)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(intent.state, GrantActivationState::Denied);
        assert_eq!(
            intent.denial,
            Some(GrantActivationDenial::AuthorityInvalidated)
        );
        assert!(!intent.capacity_held);
        let usage = harness
            .store
            .policy_usage(&harness.policy.id)
            .await
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn rotation_uniqueness_loss_retains_both_secrets_for_ambiguous_recovery() {
        let harness = default_harness().await;
        let other = harness
            .service
            .register_credential(
                &harness.sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "register-other-authority".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new("other-provider-credential").unwrap(),
                },
            )
            .await
            .unwrap();
        *harness
            .provider
            .rotation_provider_credential_id
            .lock()
            .await = Some("other-external-credential-id".into());

        assert_eq!(
            harness
                .service
                .rotate_credential(
                    &harness.sponsor,
                    RotateSponsoredCredential {
                        credential_id: harness.policy.credential_id.clone(),
                        stable_request_id: "rotation-to-existing-authority".into(),
                        replacement: ProviderCredentialSecret::new("replacement-credential")
                            .unwrap(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let state = harness.store.state.lock().await;
        let (pending, current_secret) = state
            .credentials
            .get(&harness.policy.credential_id.0)
            .unwrap();
        assert_eq!(pending.state, SponsoredCredentialState::RotationPending);
        assert_eq!(
            pending.pending_operation.as_ref().unwrap().phase,
            ProviderOperationPhase::Ambiguous
        );
        assert_eq!(current_secret.as_deref(), Some(SECRET));
        assert_eq!(
            state
                .pending_credential_secrets
                .get(&harness.policy.credential_id.0)
                .map(String::as_str),
            Some("replacement-credential")
        );
        assert_eq!(
            state.credentials.get(&other.id.0).unwrap().0.state,
            SponsoredCredentialState::Active
        );
        let active_authorities: Vec<_> = state
            .credentials
            .values()
            .filter(|(record, _)| record.state == SponsoredCredentialState::Active)
            .map(|(record, _)| {
                (
                    record.authority.provider.clone(),
                    record.authority.provider_credential_id_digest.clone(),
                )
            })
            .collect();
        assert_eq!(active_authorities.len(), 1);
    }

    #[tokio::test]
    async fn malformed_provider_evidence_never_definitively_releases_held_authority() {
        let rotation = default_harness().await;
        *rotation.provider.rotate_mode.lock().await = ProviderMode::LostResponse;
        let rotation_command = || RotateSponsoredCredential {
            credential_id: rotation.policy.credential_id.clone(),
            stable_request_id: "malformed-rotation-readback".into(),
            replacement: ProviderCredentialSecret::new("replacement-credential").unwrap(),
        };
        assert_eq!(
            rotation
                .service
                .rotate_credential(&rotation.sponsor, rotation_command())
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        *rotation.provider.rotate_mode.lock().await = ProviderMode::MalformedEvidence;
        assert_eq!(
            rotation
                .service
                .rotate_credential(&rotation.sponsor, rotation_command())
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let rotation_state = rotation.store.state.lock().await;
        let (record, secret) = rotation_state
            .credentials
            .get(&rotation.policy.credential_id.0)
            .unwrap();
        assert_eq!(record.state, SponsoredCredentialState::RotationPending);
        assert_eq!(
            record.pending_operation.as_ref().unwrap().phase,
            ProviderOperationPhase::Ambiguous
        );
        assert_eq!(secret.as_deref(), Some(SECRET));
        assert!(rotation_state
            .pending_credential_secrets
            .contains_key(&rotation.policy.credential_id.0));
        drop(rotation_state);

        let revocation = default_harness().await;
        *revocation.provider.revoke_mode.lock().await = ProviderMode::LostResponse;
        assert_eq!(
            revocation
                .service
                .revoke_credential(
                    &revocation.sponsor,
                    &revocation.policy.credential_id,
                    "malformed-revocation-readback",
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        *revocation.provider.revoke_mode.lock().await = ProviderMode::MalformedEvidence;
        assert_eq!(
            revocation
                .service
                .revoke_credential(
                    &revocation.sponsor,
                    &revocation.policy.credential_id,
                    "malformed-revocation-readback",
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let stored = revocation
            .store
            .credential(&revocation.policy.credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.record.state,
            SponsoredCredentialState::RevocationPending
        );
        assert_eq!(
            stored.record.pending_operation.as_ref().unwrap().phase,
            ProviderOperationPhase::Ambiguous
        );
        assert_eq!(
            stored.record.material_state,
            SponsoredCredentialMaterialState::Retained
        );
        assert!(stored.secret.is_some());

        let registration = default_harness().await;
        registration
            .provider
            .malformed_ownership_evidence
            .store(true, Ordering::SeqCst);
        assert_eq!(
            registration
                .service
                .register_credential(
                    &registration.sponsor,
                    RegisterSponsoredCredential {
                        stable_request_id: "malformed-registration-proof".into(),
                        owner: owner(),
                        provider: PROVIDER.into(),
                        provider_policy_key: POLICY_KEY.into(),
                        credential: ProviderCredentialSecret::new("other-provider-credential")
                            .unwrap(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderRejected
        );
        assert_eq!(registration.store.state.lock().await.credentials.len(), 1);
    }

    #[tokio::test]
    async fn invalid_confirmed_grant_receipt_is_ambiguous_and_keeps_capacity() {
        let harness = default_harness().await;
        prepare_activation(&harness).await;
        harness
            .provider
            .invalid_receipt
            .store(true, Ordering::SeqCst);
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "invalid-confirmed-receipt".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let state = harness.store.state.lock().await;
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::ProviderAmbiguous);
        assert!(intent.provider_receipt.is_some());
        assert!(intent.capacity_held);
        assert!(intent.lease.is_none());
        assert!(state.grants.is_empty());
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved.quota, harness.policy.per_launch_quota);
        assert_eq!(usage.active_launches, 1);
    }

    #[tokio::test]
    async fn only_pre_dispatch_unavailability_resets_provider_mutations() {
        let unavailable = default_harness().await;
        *unavailable.provider.rotate_mode.lock().await = ProviderMode::Unavailable;
        assert_eq!(
            unavailable
                .service
                .rotate_credential(
                    &unavailable.sponsor,
                    RotateSponsoredCredential {
                        credential_id: unavailable.policy.credential_id.clone(),
                        stable_request_id: "rotation-pre-dispatch-unavailable".into(),
                        replacement: ProviderCredentialSecret::new("replacement-credential")
                            .unwrap(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderUnavailable
        );
        let unavailable_record = unavailable
            .store
            .credential(&unavailable.policy.credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            unavailable_record
                .record
                .pending_operation
                .as_ref()
                .unwrap()
                .phase,
            ProviderOperationPhase::Prepared
        );
        assert!(unavailable_record.secret.is_some());

        *unavailable.provider.rotate_mode.lock().await = ProviderMode::MalformedEvidence;
        assert_eq!(
            unavailable
                .service
                .rotate_credential(
                    &unavailable.sponsor,
                    RotateSponsoredCredential {
                        credential_id: unavailable.policy.credential_id.clone(),
                        stable_request_id: "rotation-pre-dispatch-unavailable".into(),
                        replacement: ProviderCredentialSecret::new("replacement-credential")
                            .unwrap(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let ambiguous_record = unavailable
            .store
            .credential(&unavailable.policy.credential_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ambiguous_record
                .record
                .pending_operation
                .as_ref()
                .unwrap()
                .phase,
            ProviderOperationPhase::Ambiguous
        );
        assert!(ambiguous_record.secret.is_some());
    }

    #[tokio::test]
    async fn expired_activation_lease_can_take_over_the_same_provider_operation() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        let first = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "lease-owner-first".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "lease-owner-contender".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ActivationInProgress
        );

        harness.clock.set(LATER);
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "lease-owner-contender".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ActivationInProgress
        );
        harness.store.set_authoritative_now(LATER).await;
        harness.provider.block_issue.store(false, Ordering::SeqCst);
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "lease-owner-contender".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        harness.provider.release_issue.notify_one();
        assert!(matches!(
            first.await.unwrap(),
            Err(SponsorshipError::Store(
                SponsorshipStoreError::StaleGeneration
            ))
        ));
        let recovered = harness
            .service
            .activate_grant(
                &harness.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "lease-owner-contender".into(),
                    launch_id: harness.launch_id.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(recovered.launch_id, harness.launch_id);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.receipts.lock().await.len(), 1);
        assert_eq!(harness.provider.reconcile_calls.load(Ordering::SeqCst), 2);
        assert_eq!(harness.store.snapshot().await.grants.len(), 1);
    }

    #[tokio::test]
    async fn activation_rotation_matches_candidates_but_preserves_dispatched_operation_ids() {
        let harness = default_harness().await;
        prepare_activation(&harness).await;
        *harness.billing.mode.lock().await = BillingMode::LostResponse;
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "activation-before-key-rotation".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::BillingAmbiguous
        );
        let before = harness
            .store
            .grant_activation(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(before.state, GrantActivationState::BillingAmbiguous);
        assert!(before.lease.is_none());

        let rotated_operational = Arc::new(
            SponsorshipOperationalPseudonymizer::new(
                "fixture-rotated",
                vec![0x6a; 32],
                vec![("fixture-current".into(), vec![0x5a; 32])],
            )
            .unwrap(),
        );
        let launch = harness
            .store
            .launch_authority(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        let acceptance = harness
            .store
            .acceptance_for_launch(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        let policy_generation = before.policy_generation.to_string();
        let credential_generation = before.credential_generation.to_string();
        let rotated_provider_candidates = rotated_operational
            .pseudonymize(
                SponsorshipOperationalDomain::ActivationRequest,
                &[
                    &launch.launch_id.0,
                    &launch.contributor.0,
                    "provider",
                    &launch.digest,
                    &acceptance.digest,
                    &policy_generation,
                    &credential_generation,
                ],
            )
            .unwrap();
        assert_ne!(
            rotated_provider_candidates.current().as_str(),
            before.provider_request_id
        );
        assert!(rotated_provider_candidates
            .iter()
            .any(|candidate| candidate.as_str() == before.provider_request_id));

        *harness.provider.issue_mode.lock().await = ProviderMode::Ambiguous;
        let rotated_service = service_with_operational(&harness, rotated_operational);
        assert_eq!(
            rotated_service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "activation-after-key-rotation".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        let after = harness
            .store
            .grant_activation(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.billing_request_id, before.billing_request_id);
        assert_eq!(after.provider_request_id, before.provider_request_id);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn activation_claim_rejects_a_caller_supplied_lease() {
        let harness = default_harness().await;
        prepare_activation(&harness).await;
        *harness.billing.mode.lock().await = BillingMode::Ambiguous;
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "establish-lease-fixture".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::BillingAmbiguous
        );
        let mut candidate = harness
            .store
            .grant_activation(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        let forged_holder = harness
            .operational
            .pseudonymize(
                SponsorshipOperationalDomain::ActivationRequest,
                &[&harness.launch_id.0, "forged-holder"],
            )
            .unwrap();
        candidate.lease =
            Some(test_activation_lease(forged_holder.current().as_str().to_string(), NOW).unwrap());
        let launch = harness
            .store
            .launch_authority(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        let acceptance = harness
            .store
            .acceptance_for_launch(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            harness
                .store
                .claim_grant_activation(ClaimGrantActivation {
                    candidate,
                    holder_candidates: forged_holder.clone(),
                    billing_request_candidates: forged_holder.clone(),
                    provider_request_candidates: forged_holder,
                    launch_authority: launch,
                    acceptance,
                })
                .await,
            Err(SponsorshipStoreError::Malformed(message)) if message.contains("cannot supply a lease")
        ));
    }

    #[tokio::test]
    async fn lost_cleanup_response_retains_capacity_until_privileged_readback() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        *harness.provider.revoke_mode.lock().await = ProviderMode::LostResponse;
        let worker = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "lost-cleanup-response".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Active,
                Some(harness.session_id.clone()),
            )
            .await;
        harness.provider.release_issue.notify_one();
        assert_eq!(
            worker.await.unwrap().unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        {
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::CleanupPending);
            assert!(intent.capacity_held);
            assert_eq!(
                intent.cleanup_operation.as_ref().unwrap().phase,
                ProviderOperationPhase::Ambiguous
            );
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved.quota, harness.policy.per_launch_quota);
            assert_eq!(usage.active_launches, 1);
        }
        assert_eq!(
            harness
                .service
                .reconcile_activation_cleanup(&harness.contributor, &harness.launch_id)
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );
        harness
            .service
            .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
            .await
            .unwrap();
        let state = harness.store.state.lock().await;
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::Cleaned);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retired_credential_proof_completes_uncommitted_cleanup_without_secret_reuse() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        *harness.provider.revoke_mode.lock().await = ProviderMode::Ambiguous;
        let worker = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "cleanup-before-credential-retirement".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Active,
                Some(harness.session_id.clone()),
            )
            .await;
        harness.provider.release_issue.notify_one();
        assert_eq!(
            worker.await.unwrap().unwrap_err(),
            SponsorshipError::ProviderAmbiguous
        );
        {
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::CleanupPending);
            assert!(intent.capacity_held);
            assert!(state
                .credentials
                .get(&harness.policy.credential_id.0)
                .unwrap()
                .1
                .is_some());
        }

        *harness.provider.revoke_mode.lock().await = ProviderMode::Confirmed;
        harness
            .service
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                "retire-during-uncommitted-cleanup",
            )
            .await
            .unwrap();
        assert!(harness
            .store
            .credential(&harness.policy.credential_id)
            .await
            .unwrap()
            .unwrap()
            .secret
            .is_none());

        harness
            .service
            .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
            .await
            .unwrap();
        let state = harness.store.state.lock().await;
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::Cleaned);
        assert!(!intent.capacity_held);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 2);
        assert_eq!(harness.provider.reconcile_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn retired_credential_proof_completes_rejected_cleanup_exactly_once() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        *harness.provider.revoke_mode.lock().await = ProviderMode::Rejected;
        let worker = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "cleanup-rejected-before-retirement".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Active,
                Some(harness.session_id.clone()),
            )
            .await;
        harness.provider.release_issue.notify_one();
        assert_eq!(
            worker.await.unwrap().unwrap_err(),
            SponsorshipError::ProviderRejected
        );
        {
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::CleanupPending);
            assert!(intent.capacity_held);
            assert_eq!(
                intent.cleanup_operation.as_ref().unwrap().phase,
                ProviderOperationPhase::Rejected
            );
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved.quota, harness.policy.per_launch_quota);
            assert_eq!(usage.active_launches, 1);
        }

        *harness.provider.revoke_mode.lock().await = ProviderMode::Confirmed;
        harness
            .service
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                "retire-after-rejected-cleanup",
            )
            .await
            .unwrap();
        let provider_calls_after_retirement = harness.provider.revoke_calls.load(Ordering::SeqCst);

        harness
            .service
            .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
            .await
            .unwrap();
        harness
            .service
            .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
            .await
            .unwrap();

        let state = harness.store.state.lock().await;
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::Cleaned);
        assert!(!intent.capacity_held);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        assert_eq!(
            harness.provider.revoke_calls.load(Ordering::SeqCst),
            provider_calls_after_retirement
        );
        assert_eq!(harness.provider.reconcile_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn kill_switch_racing_provider_issue_fences_commit_and_cleans_authority() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        let worker = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "kill-switch-provider-race".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        let fence = harness
            .service
            .engage_kill_switch(&harness.admin)
            .await
            .unwrap();
        assert!(fence.kill_switch_engaged);
        {
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::ProviderAttempting);
            assert!(intent.capacity_held);
        }
        harness.provider.release_issue.notify_one();
        assert_eq!(
            worker.await.unwrap().unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::GateChanged)
        );
        let state = harness.store.state.lock().await;
        assert!(state.grants.is_empty());
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::Cleaned);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalidated_attempting_issue_uses_readback_then_cleanup_after_lost_persistence() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        let worker = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "issue-persistence-loss".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Active,
                Some(harness.session_id.clone()),
            )
            .await;
        {
            let mut state = harness.store.state.lock().await;
            let intent = state.activations.get_mut(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::ProviderAttempting);
            intent.revision += 1;
            intent.lease = None;
        }
        harness.provider.release_issue.notify_one();
        assert!(matches!(
            worker.await.unwrap(),
            Err(SponsorshipError::Store(
                SponsorshipStoreError::StaleGeneration
            ))
        ));
        {
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::ProviderAttempting);
            assert!(intent.provider_receipt.is_none());
            assert!(intent.capacity_held);
        }
        harness
            .service
            .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
            .await
            .unwrap();
        let state = harness.store.state.lock().await;
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::Cleaned);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.reconcile_calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn invalidated_billing_ambiguity_holds_capacity_until_readback_is_definitive() {
        let harness = default_harness().await;
        prepare_activation(&harness).await;
        *harness.billing.mode.lock().await = BillingMode::Ambiguous;
        assert_eq!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "ambiguous-billing-before-invalidation".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::BillingAmbiguous
        );
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Active,
                Some(harness.session_id.clone()),
            )
            .await;

        assert_eq!(
            harness
                .service
                .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
                .await
                .unwrap_err(),
            SponsorshipError::BillingAmbiguous
        );
        {
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::BillingAmbiguous);
            assert!(intent.capacity_held);
            assert!(intent.lease.is_none());
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved.quota, harness.policy.per_launch_quota);
            assert_eq!(usage.active_launches, 1);
        }

        *harness.billing.mode.lock().await = BillingMode::Rejected;
        harness
            .service
            .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
            .await
            .unwrap();
        let state = harness.store.state.lock().await;
        let intent = state.activations.get(&harness.launch_id.0).unwrap();
        assert_eq!(intent.state, GrantActivationState::Denied);
        assert_eq!(
            intent.denial,
            Some(GrantActivationDenial::AuthorityInvalidated)
        );
        assert!(!intent.capacity_held);
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        assert_eq!(harness.billing.calls.load(Ordering::SeqCst), 1);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn explicit_unavailability_releases_activation_capacity_for_retry() {
        let provider = default_harness().await;
        prepare_activation(&provider).await;
        *provider.provider.issue_mode.lock().await = ProviderMode::Unavailable;
        assert_eq!(
            provider
                .service
                .activate_grant(
                    &provider.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "provider-unavailable".into(),
                        launch_id: provider.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderUnavailable
        );
        let state = provider.store.state.lock().await;
        let usage = state.policy_usage.get(&provider.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
        drop(state);

        let billing = default_harness().await;
        prepare_activation(&billing).await;
        *billing.billing.mode.lock().await = BillingMode::Unavailable;
        assert_eq!(
            billing
                .service
                .activate_grant(
                    &billing.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "billing-unavailable".into(),
                        launch_id: billing.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::BillingUnavailable
        );
        let state = billing.store.state.lock().await;
        let usage = state.policy_usage.get(&billing.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn kill_switch_terminalizes_never_attempted_activation_states() {
        let awaiting_provider = default_harness().await;
        prepare_activation(&awaiting_provider).await;
        *awaiting_provider.provider.issue_mode.lock().await = ProviderMode::Unavailable;
        assert_eq!(
            awaiting_provider
                .service
                .activate_grant(
                    &awaiting_provider.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "awaiting-provider-before-kill".into(),
                        launch_id: awaiting_provider.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderUnavailable
        );
        assert_eq!(
            awaiting_provider
                .store
                .grant_activation(&awaiting_provider.launch_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            GrantActivationState::AwaitingProvider
        );
        awaiting_provider
            .service
            .engage_kill_switch(&awaiting_provider.admin)
            .await
            .unwrap();
        let terminal = awaiting_provider
            .store
            .grant_activation(&awaiting_provider.launch_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(terminal.state, GrantActivationState::Denied);
        assert_eq!(
            terminal.denial,
            Some(GrantActivationDenial::AuthorityInvalidated)
        );
        assert!(!terminal.capacity_held);
        assert!(terminal.lease.is_none());
        awaiting_provider
            .service
            .reconcile_activation_cleanup(&awaiting_provider.meter, &awaiting_provider.launch_id)
            .await
            .unwrap();

        let awaiting_billing = default_harness().await;
        prepare_activation(&awaiting_billing).await;
        *awaiting_billing.billing.mode.lock().await = BillingMode::Unavailable;
        assert_eq!(
            awaiting_billing
                .service
                .activate_grant(
                    &awaiting_billing.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "awaiting-billing-before-kill".into(),
                        launch_id: awaiting_billing.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::BillingUnavailable
        );
        assert_eq!(
            awaiting_billing
                .store
                .grant_activation(&awaiting_billing.launch_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            GrantActivationState::AwaitingBilling
        );
        awaiting_billing
            .service
            .engage_kill_switch(&awaiting_billing.admin)
            .await
            .unwrap();
        let terminal = awaiting_billing
            .store
            .grant_activation(&awaiting_billing.launch_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(terminal.state, GrantActivationState::Denied);
        assert_eq!(
            terminal.denial,
            Some(GrantActivationDenial::AuthorityInvalidated)
        );
        assert!(!terminal.capacity_held);
        assert!(terminal.lease.is_none());
        awaiting_billing
            .service
            .reconcile_activation_cleanup(&awaiting_billing.meter, &awaiting_billing.launch_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn acceptance_replay_precedes_dynamic_authority_and_is_caller_isolated() {
        let harness = default_harness().await;
        let accepted = accept(&harness, "acceptance-stable-replay").await;
        let launch = harness
            .store
            .launch_authority(&harness.launch_id)
            .await
            .unwrap()
            .unwrap();
        let gate_fence = harness.store.gate_fence().await.unwrap();
        let replay_candidates = harness
            .operational
            .pseudonymize(
                SponsorshipOperationalDomain::AcceptanceRequest,
                &[
                    &harness.launch_id.0,
                    &harness.contributor.subject.0,
                    "acceptance-stable-replay",
                ],
            )
            .unwrap();

        let mut concurrent_loser = accepted.clone();
        concurrent_loser.accepted_at = LATER.into();
        concurrent_loser.digest = concurrent_loser.calculate_digest().unwrap();
        concurrent_loser.validate().unwrap();
        let raced = harness
            .store
            .insert_acceptance(PersistSponsoredAcceptance {
                acceptance: concurrent_loser.clone(),
                idempotency_candidates: replay_candidates.clone(),
                launch_authority: launch.clone(),
                gate_fence: gate_fence.clone(),
                audit: SponsoredAuditEvent::privacy_safe(
                    &harness.audit,
                    SponsoredAuditAction::TermsAccepted,
                    &harness.contributor.subject,
                    SponsoredAuditTarget::Policy(harness.policy.id.clone()),
                    None,
                    "concurrent_loser",
                    LATER,
                ),
            })
            .await
            .unwrap();
        assert_eq!(raced, accepted, "the first durable acceptance wins");

        let mut replacement = accepted.clone();
        let replacement_candidates = harness
            .operational
            .pseudonymize(
                SponsorshipOperationalDomain::AcceptanceRequest,
                &[
                    &harness.launch_id.0,
                    &harness.contributor.subject.0,
                    "different-acceptance-key",
                ],
            )
            .unwrap();
        replacement.idempotency_key_hash = replacement_candidates.current().as_str().to_string();
        replacement.accepted_at = LATER.into();
        replacement.digest = replacement.calculate_digest().unwrap();
        let replacement_error = harness
            .store
            .insert_acceptance(PersistSponsoredAcceptance {
                acceptance: replacement,
                idempotency_candidates: replacement_candidates,
                launch_authority: launch.clone(),
                gate_fence: gate_fence.clone(),
                audit: SponsoredAuditEvent::privacy_safe(
                    &harness.audit,
                    SponsoredAuditAction::TermsAccepted,
                    &harness.contributor.subject,
                    SponsoredAuditTarget::Policy(harness.policy.id.clone()),
                    None,
                    "replacement_loser",
                    LATER,
                ),
            })
            .await
            .unwrap_err();
        assert_eq!(replacement_error, SponsorshipStoreError::Conflict);

        {
            let mut state = harness.store.state.lock().await;
            state
                .policy_usage
                .get_mut(&harness.policy.id.0)
                .unwrap()
                .reserved = SponsoredUsageSummary {
                quota: SponsoredQuota {
                    requests: 1,
                    input_tokens: 1,
                    output_tokens: 1,
                },
                spend_micros: 1,
            };
        }
        harness
            .service
            .engage_kill_switch(&harness.admin)
            .await
            .unwrap();
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Starting,
                Some(harness.session_id.clone()),
            )
            .await;
        harness.clock.set("not-a-canonical-timestamp");

        let delayed_store_replay = harness
            .store
            .insert_acceptance(PersistSponsoredAcceptance {
                acceptance: concurrent_loser,
                idempotency_candidates: replay_candidates,
                launch_authority: launch,
                gate_fence,
                audit: SponsoredAuditEvent::privacy_safe(
                    &harness.audit,
                    SponsoredAuditAction::TermsAccepted,
                    &harness.contributor.subject,
                    SponsoredAuditTarget::Policy(harness.policy.id.clone()),
                    None,
                    "delayed_loser",
                    LATER,
                ),
            })
            .await
            .unwrap();
        assert_eq!(delayed_store_replay, accepted);

        let replay = harness
            .service
            .accept_terms(
                &harness.contributor,
                AcceptSponsoredTerms {
                    launch_id: harness.launch_id.clone(),
                    policy_id: harness.policy.id.clone(),
                    terms_digest: harness.terms.digest.clone(),
                    launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                    idempotency_key: "acceptance-stable-replay".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(replay, accepted);

        let changed = harness
            .service
            .accept_terms(
                &harness.contributor,
                AcceptSponsoredTerms {
                    launch_id: harness.launch_id.clone(),
                    policy_id: harness.policy.id.clone(),
                    terms_digest: digest(b"changed-terms"),
                    launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                    idempotency_key: "acceptance-stable-replay".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            changed,
            SponsorshipError::Store(SponsorshipStoreError::Conflict)
        );

        let other_contributor = caller(
            SubjectId("different-launch-contributor".into()),
            [Capability::UseOwnSessions],
        );
        assert_eq!(
            harness
                .service
                .accept_terms(
                    &other_contributor,
                    AcceptSponsoredTerms {
                        launch_id: harness.launch_id.clone(),
                        policy_id: harness.policy.id.clone(),
                        terms_digest: harness.terms.digest.clone(),
                        launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                        idempotency_key: "acceptance-stable-replay".into(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::Forbidden
        );

        let state = harness.store.state.lock().await;
        assert_eq!(state.acceptances.len(), 1);
        assert_eq!(
            state
                .audits
                .iter()
                .filter(|event| event.action == SponsoredAuditAction::TermsAccepted)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn acceptance_rotation_replaces_five_aliases_without_rewriting_canonical_authority() {
        let harness = default_harness().await;
        let accepted = accept(&harness, "acceptance-five-key-replay").await;
        let original_canonical = accepted.idempotency_key_hash.clone();
        let original_digest = accepted.digest.clone();

        let five_key_operational = Arc::new(
            SponsorshipOperationalPseudonymizer::new(
                "fixture-current-4",
                vec![0x64; 32],
                vec![
                    ("fixture-current-3".into(), vec![0x63; 32]),
                    ("fixture-current-2".into(), vec![0x62; 32]),
                    ("fixture-current-1".into(), vec![0x61; 32]),
                    ("fixture-current".into(), vec![0x5a; 32]),
                ],
            )
            .unwrap(),
        );
        let five_key_candidates = five_key_operational
            .pseudonymize(
                SponsorshipOperationalDomain::AcceptanceRequest,
                &[
                    &harness.launch_id.0,
                    &harness.contributor.subject.0,
                    "acceptance-five-key-replay",
                ],
            )
            .unwrap();
        assert_eq!(five_key_candidates.iter().count(), 5);
        let rotated_service = service_with_operational(&harness, five_key_operational);
        let replay = rotated_service
            .accept_terms(
                &harness.contributor,
                AcceptSponsoredTerms {
                    launch_id: harness.launch_id.clone(),
                    policy_id: harness.policy.id.clone(),
                    terms_digest: harness.terms.digest.clone(),
                    launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                    idempotency_key: "acceptance-five-key-replay".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(replay.idempotency_key_hash, original_canonical);
        assert_eq!(replay.digest, original_digest);
        {
            let state = harness.store.state.lock().await;
            assert_eq!(
                state.acceptance_aliases.get(&harness.launch_id.0).unwrap(),
                &pseudonym_aliases(&five_key_candidates)
            );
            assert_eq!(
                state
                    .acceptance_aliases
                    .get(&harness.launch_id.0)
                    .unwrap()
                    .len(),
                5
            );
        }

        let without_canonical = Arc::new(
            SponsorshipOperationalPseudonymizer::new(
                "fixture-current-5",
                vec![0x65; 32],
                vec![
                    ("fixture-current-4".into(), vec![0x64; 32]),
                    ("fixture-current-3".into(), vec![0x63; 32]),
                    ("fixture-current-2".into(), vec![0x62; 32]),
                    ("fixture-current-1".into(), vec![0x61; 32]),
                ],
            )
            .unwrap(),
        );
        let refused = service_with_operational(&harness, without_canonical)
            .accept_terms(
                &harness.contributor,
                AcceptSponsoredTerms {
                    launch_id: harness.launch_id.clone(),
                    policy_id: harness.policy.id.clone(),
                    terms_digest: harness.terms.digest.clone(),
                    launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                    idempotency_key: "acceptance-five-key-replay".into(),
                },
            )
            .await;
        assert!(matches!(
            refused,
            Err(SponsorshipError::Store(SponsorshipStoreError::Conflict))
        ));
        let state = harness.store.state.lock().await;
        assert_eq!(
            state
                .acceptance_aliases
                .get(&harness.launch_id.0)
                .unwrap()
                .len(),
            5
        );
    }

    #[tokio::test]
    async fn revoked_policy_is_terminal_across_suspend_revoke_races() {
        let sequential = default_harness().await;
        let suspended = sequential
            .service
            .suspend_policy(&sequential.sponsor, &sequential.policy.id)
            .await
            .unwrap();
        assert_eq!(suspended.state, SponsorshipPolicyState::Suspended);
        assert!(suspended.revoked_at.is_none());
        let sequentially_revoked = sequential
            .service
            .revoke_policy(&sequential.sponsor, &sequential.policy.id)
            .await
            .unwrap();
        assert_eq!(sequentially_revoked.state, SponsorshipPolicyState::Revoked);

        let harness = Arc::new(default_harness().await);
        let suspend = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .suspend_policy(&harness.sponsor, &harness.policy.id)
                    .await
            })
        };
        let revoke = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .revoke_policy(&harness.sponsor, &harness.policy.id)
                    .await
            })
        };
        let suspended_or_revoked = suspend.await.unwrap().unwrap();
        assert!(matches!(
            suspended_or_revoked.state,
            SponsorshipPolicyState::Suspended | SponsorshipPolicyState::Revoked
        ));
        let revoked = revoke.await.unwrap().unwrap();
        assert_eq!(revoked.state, SponsorshipPolicyState::Revoked);
        assert!(revoked.revoked_at.is_some());

        let terminal_retry = harness
            .service
            .suspend_policy(&harness.sponsor, &harness.policy.id)
            .await
            .unwrap();
        assert_eq!(terminal_retry, revoked);
        let final_policy = harness
            .store
            .policy(&harness.policy.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_policy, revoked);

        let mut missing_timestamp = revoked.clone();
        missing_timestamp.revoked_at = None;
        assert!(missing_timestamp.validate().is_err());
        let mut stray_timestamp = revoked;
        stray_timestamp.state = SponsorshipPolicyState::Suspended;
        assert!(stray_timestamp.validate().is_err());
    }

    #[tokio::test]
    async fn policy_state_change_stops_after_bounded_stale_generation_contention() {
        let harness = default_harness().await;
        harness
            .store
            .policy_replace_stale_failures
            .store(MAX_POLICY_STATE_CHANGE_ATTEMPTS + 1, Ordering::SeqCst);

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            harness
                .service
                .suspend_policy(&harness.sponsor, &harness.policy.id),
        )
        .await
        .expect("bounded policy CAS must terminate")
        .unwrap_err();

        assert_eq!(
            error,
            SponsorshipError::Store(SponsorshipStoreError::StaleGeneration)
        );
        assert_eq!(
            harness.store.policy_replace_attempts.load(Ordering::SeqCst),
            MAX_POLICY_STATE_CHANGE_ATTEMPTS
        );
        assert_eq!(
            harness
                .store
                .policy_replace_stale_failures
                .load(Ordering::SeqCst),
            1
        );
        let state = harness.store.state.lock().await;
        assert_eq!(
            state.policies.get(&harness.policy.id.0).unwrap().state,
            SponsorshipPolicyState::Active
        );
        assert!(state
            .audits
            .iter()
            .all(|event| event.action != SponsoredAuditAction::PolicySuspended));
    }

    #[tokio::test]
    async fn registration_key_binds_provider_policy_authority() {
        let harness = default_harness().await;
        harness.clock.set(LATER);
        let same = harness
            .service
            .register_credential(
                &harness.sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "register-credential-a".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new(SECRET).unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(same.id, harness.policy.credential_id);

        let changed = harness
            .service
            .register_credential(
                &harness.sponsor,
                RegisterSponsoredCredential {
                    stable_request_id: "register-credential-a".into(),
                    owner: owner(),
                    provider: PROVIDER.into(),
                    provider_policy_key: ALTERNATE_POLICY_KEY.into(),
                    credential: ProviderCredentialSecret::new(SECRET).unwrap(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(
            changed,
            SponsorshipError::Store(SponsorshipStoreError::Conflict)
        );
        assert_eq!(harness.store.state.lock().await.credentials.len(), 1);
    }

    #[tokio::test]
    async fn activation_commit_transfers_one_existing_capacity_hold() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        let activation = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "capacity-owner-transfer".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        let held = {
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert!(intent.capacity_held);
            state
                .policy_usage
                .get(&harness.policy.id.0)
                .unwrap()
                .clone()
        };
        assert_eq!(held.reserved.quota, harness.policy.per_launch_quota);
        assert_eq!(
            held.reserved.spend_micros,
            harness.policy.per_launch_budget_micros
        );
        assert_eq!(held.active_launches, 1);

        harness.provider.release_issue.notify_one();
        let grant = activation.await.unwrap().unwrap();
        {
            let state = harness.store.state.lock().await;
            assert_eq!(state.policy_usage.get(&harness.policy.id.0).unwrap(), &held);
            assert!(
                !state
                    .activations
                    .get(&harness.launch_id.0)
                    .unwrap()
                    .capacity_held
            );
            assert_eq!(
                state.grants.get(&grant.id.0).unwrap().allocation,
                held.reserved
            );
        }

        let closed = harness
            .service
            .revoke_grant(&harness.sponsor, &grant.id, "capacity-release-once")
            .await
            .unwrap();
        let replay = harness
            .service
            .revoke_grant(&harness.sponsor, &grant.id, "capacity-release-once")
            .await
            .unwrap();
        assert_eq!(replay, closed);
        let usage = harness
            .store
            .policy_usage(&harness.policy.id)
            .await
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn aggregate_concurrency_and_tenant_fences_fail_before_external_observation() {
        let quota_limited = harness(Limits {
            aggregate_quota: Limits::default().per_launch_quota,
            ..Limits::default()
        })
        .await;
        prepare_activation(&quota_limited).await;
        activate(&quota_limited, "aggregate-quota-first").await;
        let (second_launch, _) =
            prepare_additional_activation(&quota_limited, "aggregate-quota").await;
        assert_eq!(
            quota_limited
                .service
                .activate_grant(
                    &quota_limited.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "aggregate-quota-second".into(),
                        launch_id: second_launch,
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::AggregateQuotaExceeded)
        );
        assert_eq!(quota_limited.provider.issue_calls.load(Ordering::SeqCst), 1);

        let budget_limited = harness(Limits {
            aggregate_budget_micros: Limits::default().per_launch_budget_micros,
            ..Limits::default()
        })
        .await;
        prepare_activation(&budget_limited).await;
        activate(&budget_limited, "aggregate-budget-first").await;
        let (second_launch, _) =
            prepare_additional_activation(&budget_limited, "aggregate-budget").await;
        assert_eq!(
            budget_limited
                .service
                .activate_grant(
                    &budget_limited.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "aggregate-budget-second".into(),
                        launch_id: second_launch,
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::AggregateBudgetExceeded)
        );

        let concurrency_limited = harness(Limits {
            max_concurrent_launches: 1,
            ..Limits::default()
        })
        .await;
        prepare_activation(&concurrency_limited).await;
        activate(&concurrency_limited, "concurrency-first").await;
        let (second_launch, _) =
            prepare_additional_activation(&concurrency_limited, "concurrency").await;
        assert_eq!(
            concurrency_limited
                .service
                .activate_grant(
                    &concurrency_limited.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "concurrency-second".into(),
                        launch_id: second_launch,
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::ConcurrentLaunchLimit)
        );

        let tenant_mismatch = default_harness().await;
        accept(&tenant_mismatch, "tenant-mismatch-acceptance").await;
        let mut launch = tenant_mismatch
            .store
            .launch_authority(&tenant_mismatch.launch_id)
            .await
            .unwrap()
            .unwrap();
        launch.tenant_id = "tenant-b".into();
        launch.state = SponsoredLaunchState::Starting;
        launch.session_id = Some(tenant_mismatch.session_id.clone());
        launch.generation += 1;
        launch.digest = launch.calculate_digest().unwrap();
        tenant_mismatch.store.put_launch(launch).await;
        assert_eq!(
            tenant_mismatch
                .service
                .activate_grant(
                    &tenant_mismatch.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "tenant-mismatch-activation".into(),
                        launch_id: tenant_mismatch.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::PolicyUnavailable
        );
        assert_eq!(tenant_mismatch.billing.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            tenant_mismatch.provider.issue_calls.load(Ordering::SeqCst),
            0
        );
    }

    #[tokio::test]
    async fn usage_rate_window_uses_authoritative_database_time() {
        let harness = harness(Limits {
            max_requests_per_minute: 1,
            ..Limits::default()
        })
        .await;
        let grant = activate_for_usage(&harness).await;
        let quota = SponsoredQuota {
            requests: 1,
            input_tokens: 10,
            output_tokens: 10,
        };
        harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "database-rate-first", quota),
            )
            .await
            .unwrap();
        harness.clock.set(LATER);
        assert_eq!(
            harness
                .service
                .reserve_usage(
                    &harness.contributor,
                    usage_command(&grant, "database-rate-still-held", quota),
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::RateExceeded)
        );
        harness.store.set_authoritative_now(LATER).await;
        harness
            .service
            .reserve_usage(
                &harness.contributor,
                usage_command(&grant, "database-rate-window-advanced", quota),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn activation_refreshes_clock_after_provider_io_before_receipt_validation() {
        let harness = Arc::new(default_harness().await);
        prepare_activation(&harness).await;
        *harness.provider.receipt_issued_at.lock().await = LATER.into();
        harness.provider.block_issue.store(true, Ordering::SeqCst);
        let activation = {
            let harness = harness.clone();
            tokio::spawn(async move {
                harness
                    .service
                    .activate_grant(
                        &harness.contributor,
                        ActivateSponsoredGrant {
                            stable_request_id: "completion-time-provider-receipt".into(),
                            launch_id: harness.launch_id.clone(),
                        },
                    )
                    .await
            })
        };
        harness.provider.issue_entered.notified().await;
        harness.clock.set(LATER);
        harness.store.set_authoritative_now(LATER).await;
        harness.provider.release_issue.notify_one();

        let grant = activation.await.unwrap().unwrap();
        assert_eq!(grant.authority.issued_at, LATER);
        assert_eq!(grant.created_at, LATER);
        assert_eq!(grant.updated_at, LATER);
    }

    #[tokio::test]
    async fn a_provider_receipt_cannot_backdate_issuance_before_acceptance() {
        let harness = default_harness().await;
        prepare_activation(&harness).await;
        *harness.provider.receipt_issued_at.lock().await = EARLIER.into();

        assert!(matches!(
            harness
                .service
                .activate_grant(
                    &harness.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "pre-acceptance-provider-issuance".into(),
                        launch_id: harness.launch_id.clone(),
                    },
                )
                .await,
            Err(SponsorshipError::Validation(_))
        ));
        assert!(harness.store.snapshot().await.grants.is_empty());
    }

    #[tokio::test]
    async fn database_time_expiry_during_provider_io_rejects_commit_and_cleans_once() {
        for (database_now, request_id) in [
            (GRANT_EXPIRY, "receipt-expires-during-provider-io"),
            (POLICY_EXPIRY, "policy-expires-during-provider-io"),
        ] {
            let harness = Arc::new(default_harness().await);
            prepare_activation(&harness).await;
            harness.provider.block_issue.store(true, Ordering::SeqCst);
            let activation = {
                let harness = harness.clone();
                tokio::spawn(async move {
                    harness
                        .service
                        .activate_grant(
                            &harness.contributor,
                            ActivateSponsoredGrant {
                                stable_request_id: request_id.into(),
                                launch_id: harness.launch_id.clone(),
                            },
                        )
                        .await
                })
            };
            harness.provider.issue_entered.notified().await;
            harness.store.set_authoritative_now(database_now).await;
            harness.clock.set(LATER);
            harness.provider.release_issue.notify_one();

            assert_eq!(
                activation.await.unwrap().unwrap_err(),
                SponsorshipError::Store(SponsorshipStoreError::GrantUnavailable)
            );
            let state = harness.store.state.lock().await;
            assert!(state.grants.is_empty());
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::Cleaned);
            assert!(!intent.capacity_held);
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved, SponsoredUsageSummary::default());
            assert_eq!(usage.active_launches, 0);
            drop(state);
            assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 1);
            assert_eq!(harness.provider.revoke_calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn usage_expiry_cas_uses_database_time_not_service_request_time() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        harness.store.set_authoritative_now(GRANT_EXPIRY).await;
        assert_eq!(
            harness
                .service
                .reserve_usage(
                    &harness.contributor,
                    usage_command(
                        &grant,
                        "database-expired-usage",
                        SponsoredQuota {
                            requests: 1,
                            input_tokens: 10,
                            output_tokens: 10,
                        },
                    ),
                )
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::GrantUnavailable)
        );
        assert!(harness.store.state.lock().await.reservations.is_empty());
    }

    #[tokio::test]
    async fn billing_confirmation_revalidates_every_authority_before_provider_dispatch() {
        #[derive(Clone, Copy)]
        enum Invalidation {
            KillSwitch,
            Launch,
            Acceptance,
            PolicyRevocation,
            PolicyExpiry,
            CredentialRotation,
            CredentialRevocation,
        }

        for invalidation in [
            Invalidation::KillSwitch,
            Invalidation::Launch,
            Invalidation::Acceptance,
            Invalidation::PolicyRevocation,
            Invalidation::PolicyExpiry,
            Invalidation::CredentialRotation,
            Invalidation::CredentialRevocation,
        ] {
            let harness = Arc::new(default_harness().await);
            prepare_activation(&harness).await;
            harness.billing.block_confirm.store(true, Ordering::SeqCst);
            let activation = {
                let harness = harness.clone();
                tokio::spawn(async move {
                    harness
                        .service
                        .activate_grant(
                            &harness.contributor,
                            ActivateSponsoredGrant {
                                stable_request_id: "billing-authority-race".into(),
                                launch_id: harness.launch_id.clone(),
                            },
                        )
                        .await
                })
            };
            harness.billing.confirm_entered.notified().await;

            match invalidation {
                Invalidation::KillSwitch => {
                    harness
                        .service
                        .engage_kill_switch(&harness.admin)
                        .await
                        .unwrap();
                }
                Invalidation::Launch => {
                    harness
                        .store
                        .update_launch_state(
                            &harness.launch_id,
                            SponsoredLaunchState::Active,
                            Some(harness.session_id.clone()),
                        )
                        .await;
                }
                Invalidation::Acceptance => {
                    let mut state = harness.store.state.lock().await;
                    let acceptance = state.acceptances.get_mut(&harness.launch_id.0).unwrap();
                    acceptance.accepted_at = LATER.into();
                    acceptance.digest = acceptance.calculate_digest().unwrap();
                }
                Invalidation::PolicyRevocation => {
                    harness
                        .service
                        .revoke_policy(&harness.sponsor, &harness.policy.id)
                        .await
                        .unwrap();
                }
                Invalidation::PolicyExpiry => {
                    harness.store.set_authoritative_now(POLICY_EXPIRY).await;
                }
                Invalidation::CredentialRotation => {
                    harness
                        .service
                        .rotate_credential(
                            &harness.sponsor,
                            RotateSponsoredCredential {
                                credential_id: harness.policy.credential_id.clone(),
                                stable_request_id: "billing-race-rotate-root".into(),
                                replacement: ProviderCredentialSecret::new(
                                    "replacement-credential",
                                )
                                .unwrap(),
                            },
                        )
                        .await
                        .unwrap();
                }
                Invalidation::CredentialRevocation => {
                    harness
                        .service
                        .revoke_credential(
                            &harness.sponsor,
                            &harness.policy.credential_id,
                            "billing-race-revoke-root",
                        )
                        .await
                        .unwrap();
                }
            }
            harness.billing.release_confirm.notify_one();

            assert_eq!(
                activation.await.unwrap().unwrap_err(),
                SponsorshipError::ScopeDenied
            );
            assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 0);
            let state = harness.store.state.lock().await;
            let intent = state.activations.get(&harness.launch_id.0).unwrap();
            assert_eq!(intent.state, GrantActivationState::Denied);
            assert_eq!(
                intent.denial,
                Some(GrantActivationDenial::AuthorityInvalidated)
            );
            assert!(!intent.capacity_held);
            assert!(intent.lease.is_none());
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved, SponsoredUsageSummary::default());
            assert_eq!(usage.active_launches, 0);
            drop(state);

            harness
                .service
                .reconcile_activation_cleanup(&harness.meter, &harness.launch_id)
                .await
                .unwrap();
            let usage = harness
                .store
                .state
                .lock()
                .await
                .policy_usage
                .get(&harness.policy.id.0)
                .cloned()
                .unwrap();
            assert_eq!(usage.reserved, SponsoredUsageSummary::default());
            assert_eq!(usage.active_launches, 0);
        }
    }

    #[tokio::test]
    async fn rotation_replay_uses_durable_history_after_later_successes() {
        let harness = default_harness().await;
        let first = RotateSponsoredCredential {
            credential_id: harness.policy.credential_id.clone(),
            stable_request_id: "rotation-ledger-r1".into(),
            replacement: ProviderCredentialSecret::new("replacement-credential").unwrap(),
        };
        let first_view = harness
            .service
            .rotate_credential(&harness.sponsor, first)
            .await
            .unwrap();
        let second = RotateSponsoredCredential {
            credential_id: harness.policy.credential_id.clone(),
            stable_request_id: "rotation-ledger-r2".into(),
            replacement: ProviderCredentialSecret::new("replacement-credential-2").unwrap(),
        };
        let second_view = harness
            .service
            .rotate_credential(&harness.sponsor, second)
            .await
            .unwrap();
        assert!(second_view.generation > first_view.generation);
        assert_eq!(harness.provider.rotate_calls.load(Ordering::SeqCst), 2);

        let delayed = harness
            .service
            .rotate_credential(
                &harness.sponsor,
                RotateSponsoredCredential {
                    credential_id: harness.policy.credential_id.clone(),
                    stable_request_id: "rotation-ledger-r1".into(),
                    replacement: ProviderCredentialSecret::new("replacement-credential").unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(delayed, second_view);
        assert!(matches!(
            harness
                .service
                .rotate_credential(
                    &harness.sponsor,
                    RotateSponsoredCredential {
                        credential_id: harness.policy.credential_id.clone(),
                        stable_request_id: "rotation-ledger-r1".into(),
                        replacement: ProviderCredentialSecret::new("replacement-credential-2")
                            .unwrap(),
                    },
                )
                .await,
            Err(SponsorshipError::Store(SponsorshipStoreError::Conflict))
        ));
        assert_eq!(harness.provider.rotate_calls.load(Ordering::SeqCst), 2);
        let state = harness.store.state.lock().await;
        assert_eq!(state.credential_operations.len(), 2);
        assert!(state.credential_operations.values().all(|operation| {
            operation.kind == ProviderOperationKind::RotateCredential
                && operation.outcome == CompletedProviderOperationOutcome::Confirmed
        }));
        drop(state);

        let pending = Arc::new(default_harness().await);
        pending.provider.block_rotate.store(true, Ordering::SeqCst);
        let worker = {
            let pending = pending.clone();
            tokio::spawn(async move {
                pending
                    .service
                    .rotate_credential(
                        &pending.sponsor,
                        RotateSponsoredCredential {
                            credential_id: pending.policy.credential_id.clone(),
                            stable_request_id: "rotation-pending-binding".into(),
                            replacement: ProviderCredentialSecret::new("replacement-credential")
                                .unwrap(),
                        },
                    )
                    .await
            })
        };
        pending.provider.rotate_entered.notified().await;
        assert!(matches!(
            pending
                .service
                .rotate_credential(
                    &pending.sponsor,
                    RotateSponsoredCredential {
                        credential_id: pending.policy.credential_id.clone(),
                        stable_request_id: "rotation-pending-binding".into(),
                        replacement: ProviderCredentialSecret::new("replacement-credential-2")
                            .unwrap(),
                    },
                )
                .await,
            Err(SponsorshipError::Store(SponsorshipStoreError::Conflict))
        ));
        assert_eq!(pending.provider.rotate_calls.load(Ordering::SeqCst), 1);
        pending.provider.release_rotate.notify_one();
        worker.await.unwrap().unwrap();
        assert_eq!(pending.provider.rotate_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn live_capacity_does_not_change_accepted_terms_identity() {
        let harness = default_harness().await;
        let second_launch = LaunchId::new();
        let second_session = SessionId("session-concurrent-terms".into());
        harness
            .store
            .put_launch(launch_authority(
                &harness.policy,
                second_launch.clone(),
                harness.contributor.subject.clone(),
            ))
            .await;
        let second_terms = harness
            .service
            .terms(&harness.contributor, &second_launch)
            .await
            .unwrap();
        assert_eq!(harness.terms.digest, second_terms.digest);

        let first_accept = harness.service.accept_terms(
            &harness.contributor,
            AcceptSponsoredTerms {
                launch_id: harness.launch_id.clone(),
                policy_id: harness.policy.id.clone(),
                terms_digest: harness.terms.digest.clone(),
                launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                idempotency_key: "concurrent-terms-first".into(),
            },
        );
        let second_accept = harness.service.accept_terms(
            &harness.contributor,
            AcceptSponsoredTerms {
                launch_id: second_launch.clone(),
                policy_id: harness.policy.id.clone(),
                terms_digest: second_terms.digest,
                launch_snapshot_digest: digest(b"fixture-launch-snapshot"),
                idempotency_key: "concurrent-terms-second".into(),
            },
        );
        let (first_accept, second_accept) = tokio::join!(first_accept, second_accept);
        first_accept.unwrap();
        second_accept.unwrap();
        harness
            .store
            .update_launch_state(
                &harness.launch_id,
                SponsoredLaunchState::Starting,
                Some(harness.session_id.clone()),
            )
            .await;
        harness
            .store
            .update_launch_state(
                &second_launch,
                SponsoredLaunchState::Starting,
                Some(second_session),
            )
            .await;

        let first_activation = harness.service.activate_grant(
            &harness.contributor,
            ActivateSponsoredGrant {
                stable_request_id: "concurrent-terms-grant-first".into(),
                launch_id: harness.launch_id.clone(),
            },
        );
        let second_activation = harness.service.activate_grant(
            &harness.contributor,
            ActivateSponsoredGrant {
                stable_request_id: "concurrent-terms-grant-second".into(),
                launch_id: second_launch.clone(),
            },
        );
        let (first_grant, second_grant) = tokio::join!(first_activation, second_activation);
        assert_eq!(first_grant.unwrap().launch_id, harness.launch_id);
        assert_eq!(second_grant.unwrap().launch_id, second_launch);
        assert_eq!(harness.provider.issue_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn capacity_changes_do_not_break_provider_or_billing_outage_retries() {
        let provider = default_harness().await;
        prepare_activation(&provider).await;
        *provider.provider.issue_mode.lock().await = ProviderMode::Unavailable;
        assert_eq!(
            provider
                .service
                .activate_grant(
                    &provider.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "stable-terms-provider-outage".into(),
                        launch_id: provider.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::ProviderUnavailable
        );
        *provider.provider.issue_mode.lock().await = ProviderMode::Confirmed;
        let (other_launch, _) = prepare_additional_activation(&provider, "provider-outage").await;
        provider
            .service
            .activate_grant(
                &provider.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "provider-outage-other".into(),
                    launch_id: other_launch,
                },
            )
            .await
            .unwrap();
        assert!(provider
            .service
            .activate_grant(
                &provider.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "stable-terms-provider-outage".into(),
                    launch_id: provider.launch_id.clone(),
                },
            )
            .await
            .is_ok());

        let billing = default_harness().await;
        prepare_activation(&billing).await;
        *billing.billing.mode.lock().await = BillingMode::Unavailable;
        assert_eq!(
            billing
                .service
                .activate_grant(
                    &billing.contributor,
                    ActivateSponsoredGrant {
                        stable_request_id: "stable-terms-billing-outage".into(),
                        launch_id: billing.launch_id.clone(),
                    },
                )
                .await
                .unwrap_err(),
            SponsorshipError::BillingUnavailable
        );
        *billing.billing.mode.lock().await = BillingMode::Confirmed;
        let (other_launch, _) = prepare_additional_activation(&billing, "billing-outage").await;
        billing
            .service
            .activate_grant(
                &billing.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "billing-outage-other".into(),
                    launch_id: other_launch,
                },
            )
            .await
            .unwrap();
        assert!(billing
            .service
            .activate_grant(
                &billing.contributor,
                ActivateSponsoredGrant {
                    stable_request_id: "stable-terms-billing-outage".into(),
                    launch_id: billing.launch_id.clone(),
                },
            )
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn grant_expiry_release_is_authorized_only_by_database_time() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        harness.clock.set(GRANT_EXPIRY);
        assert_eq!(
            harness
                .service
                .expire_grant(&harness.meter, &grant.id)
                .await
                .unwrap_err(),
            SponsorshipError::Store(SponsorshipStoreError::GrantUnavailable)
        );
        {
            let state = harness.store.state.lock().await;
            assert_eq!(
                state.grants.get(&grant.id.0).unwrap().state,
                SponsoredGrantState::Active
            );
            let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
            assert_eq!(usage.reserved, grant.allocation);
            assert_eq!(usage.active_launches, 1);
        }

        harness.store.set_authoritative_now(GRANT_EXPIRY).await;
        harness.clock.set(NOW);
        let expired = harness
            .service
            .expire_grant(&harness.meter, &grant.id)
            .await
            .unwrap();
        assert_eq!(expired.state, SponsoredGrantState::Expired);
        let state = harness.store.state.lock().await;
        let usage = state.policy_usage.get(&harness.policy.id.0).unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }

    #[tokio::test]
    async fn global_root_revocation_supersedes_rejected_grant_revocation() {
        let harness = default_harness().await;
        let grant = activate_for_usage(&harness).await;
        *harness.provider.revoke_mode.lock().await = ProviderMode::Rejected;
        assert_eq!(
            harness
                .service
                .revoke_grant(&harness.sponsor, &grant.id, "grant-rejected-before-root")
                .await
                .unwrap_err(),
            SponsorshipError::ProviderRejected
        );
        let reads_after_rejection = harness.store.provider_receipt_reads.load(Ordering::SeqCst);
        assert_eq!(reads_after_rejection, 1);

        *harness.provider.revoke_mode.lock().await = ProviderMode::Confirmed;
        harness
            .service
            .revoke_credential(
                &harness.sponsor,
                &harness.policy.credential_id,
                "root-after-grant-rejection",
            )
            .await
            .unwrap();
        let provider_calls_after_root = harness.provider.revoke_calls.load(Ordering::SeqCst);
        let revoked = harness
            .service
            .revoke_grant(&harness.sponsor, &grant.id, "grant-rejected-before-root")
            .await
            .unwrap();
        assert_eq!(revoked.state, SponsoredGrantState::Revoked);
        assert_eq!(
            harness.provider.revoke_calls.load(Ordering::SeqCst),
            provider_calls_after_root
        );
        assert_eq!(
            harness.store.provider_receipt_reads.load(Ordering::SeqCst),
            reads_after_rejection
        );
        let usage = harness
            .store
            .state
            .lock()
            .await
            .policy_usage
            .get(&harness.policy.id.0)
            .cloned()
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);

        let replay = harness
            .service
            .revoke_grant(&harness.sponsor, &grant.id, "grant-rejected-before-root")
            .await
            .unwrap();
        assert_eq!(replay, revoked);
        let usage = harness
            .store
            .state
            .lock()
            .await
            .policy_usage
            .get(&harness.policy.id.0)
            .cloned()
            .unwrap();
        assert_eq!(usage.reserved, SponsoredUsageSummary::default());
        assert_eq!(usage.active_launches, 0);
    }
}
