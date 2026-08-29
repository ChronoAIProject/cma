//! Credential, policy, acceptance, activation, and revocation lifecycles.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::super::{LaunchId, SponsoredPolicyBinding, TriggerValidationError};
use super::model::*;
use super::ports::*;
use super::validation::*;
use crate::{Caller, Capability, SubjectId};

/// Deployment-owned proof that provider expiry and revocation were
/// demonstrated, plus absolute ceilings no publisher may exceed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipGate {
    pub enabled: bool,
    pub expiry_evidence_id: Option<String>,
    pub revocation_evidence_id: Option<String>,
    pub allowed_provider_policy_keys: BTreeSet<String>,
    pub maximum_policy_budget_micros: u64,
    pub maximum_policy_requests: u64,
    pub maximum_concurrent_launches: u32,
    pub kill_switch_engaged: bool,
}

impl SponsorshipGate {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            expiry_evidence_id: None,
            revocation_evidence_id: None,
            allowed_provider_policy_keys: BTreeSet::new(),
            maximum_policy_budget_micros: 0,
            maximum_policy_requests: 0,
            maximum_concurrent_launches: 0,
            kill_switch_engaged: true,
        }
    }

    pub fn validate(&self) -> Result<(), SponsorshipError> {
        if !self.enabled {
            return Err(SponsorshipError::Disabled);
        }
        if self.kill_switch_engaged {
            return Err(SponsorshipError::KillSwitch);
        }
        let evidence_is_usable = |value: &Option<String>| {
            value.as_deref().is_some_and(|value| {
                value.len() >= 8
                    && value.len() <= 255
                    && value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.')
                    })
            })
        };
        if !evidence_is_usable(&self.expiry_evidence_id)
            || !evidence_is_usable(&self.revocation_evidence_id)
        {
            return Err(SponsorshipError::CapabilityEvidenceMissing);
        }
        if self.allowed_provider_policy_keys.is_empty()
            || self.maximum_policy_budget_micros == 0
            || self.maximum_policy_requests == 0
            || self.maximum_concurrent_launches == 0
        {
            return Err(SponsorshipError::InvalidGate);
        }
        for key in &self.allowed_provider_policy_keys {
            validate_policy_token("allowed_provider_policy_keys", key)?;
        }
        Ok(())
    }

    pub fn configuration_digest(&self) -> Result<String, TriggerValidationError> {
        canonical_digest(&(
            "cma-sponsorship-gate-v1",
            self.enabled,
            &self.expiry_evidence_id,
            &self.revocation_evidence_id,
            &self.allowed_provider_policy_keys,
            self.maximum_policy_budget_micros,
            self.maximum_policy_requests,
            self.maximum_concurrent_launches,
        ))
    }

    pub fn require_fence(
        &self,
        fence: SponsorshipGateFence,
    ) -> Result<SponsorshipGateFence, SponsorshipError> {
        self.validate()?;
        fence.validate()?;
        if fence.configuration_digest != self.configuration_digest()? {
            return Err(SponsorshipError::InvalidGate);
        }
        if fence.kill_switch_engaged {
            return Err(SponsorshipError::KillSwitch);
        }
        Ok(fence)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SponsorshipError {
    #[error("sponsorship is disabled")]
    Disabled,
    #[error("the sponsorship emergency kill switch is engaged")]
    KillSwitch,
    #[error("provider expiry and revocation evidence is not configured")]
    CapabilityEvidenceMissing,
    #[error("the sponsorship deployment gate is incomplete")]
    InvalidGate,
    #[error("the sponsored resource was not found")]
    NotFound,
    #[error("the caller does not control the sponsored resource")]
    Forbidden,
    #[error("the sponsored policy is expired, suspended, or revoked")]
    PolicyUnavailable,
    #[error("the sponsored credential is not active")]
    CredentialUnavailable,
    #[error("the provider policy is not deployment-allowlisted")]
    ProviderPolicyDenied,
    #[error("the contributor has not accepted the current sponsored terms")]
    TermsNotAccepted,
    #[error("the accepted sponsored terms changed")]
    TermsChanged,
    #[error("billing did not confirm this grant")]
    BillingRejected,
    #[error("billing confirmation is ambiguous; no grant was activated")]
    BillingAmbiguous,
    #[error("billing is unavailable; no grant was activated")]
    BillingUnavailable,
    #[error("the provider operation is ambiguous; no grant was activated")]
    ProviderAmbiguous,
    #[error("the provider is unavailable")]
    ProviderUnavailable,
    #[error("the provider rejected this sponsored operation")]
    ProviderRejected,
    #[error("trusted sponsored pricing is unavailable")]
    PricingUnavailable,
    #[error("the selected provider, model, or resource has no trusted sponsored price")]
    PricingUnsupported,
    #[error("another caller owns this provider mutation; retry after reconciliation")]
    ProviderOperationInProgress,
    #[error("another request owns the sponsored grant activation; retry by reading the Launch")]
    ActivationInProgress,
    #[error("the sponsored request does not match its tenant, revision, Launch, session, provider, model, or resource policy")]
    ScopeDenied,
    #[error("the sponsored grant is expired, revoked, suspended, or exhausted")]
    GrantUnavailable,
    #[error("sponsorship input is invalid: {0}")]
    Validation(#[from] TriggerValidationError),
    #[error("sponsorship persistence failed: {0}")]
    Store(#[from] SponsorshipStoreError),
}

pub struct RegisterSponsoredCredential {
    pub stable_request_id: String,
    pub owner: SponsorBillingOwner,
    pub provider: String,
    pub provider_policy_key: String,
    pub credential: ProviderCredentialSecret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSponsorshipPolicy {
    pub tenant_id: String,
    pub owner: SponsorBillingOwner,
    pub credential_id: SponsoredCredentialId,
    pub provider_policy: SponsoredProviderPolicy,
    pub per_launch_quota: SponsoredQuota,
    pub aggregate_quota: SponsoredQuota,
    pub per_launch_budget_micros: u64,
    pub aggregate_budget_micros: u64,
    pub max_requests_per_minute: u32,
    pub max_concurrent_launches: u32,
    pub expires_at: String,
}

pub struct RotateSponsoredCredential {
    pub credential_id: SponsoredCredentialId,
    pub stable_request_id: String,
    pub replacement: ProviderCredentialSecret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptSponsoredTerms {
    pub launch_id: LaunchId,
    pub policy_id: SponsorshipPolicyId,
    pub terms_digest: String,
    pub launch_snapshot_digest: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivateSponsoredGrant {
    pub stable_request_id: String,
    pub launch_id: LaunchId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSponsoredUsage {
    pub stable_request_id: String,
    pub grant_id: SponsoredGrantId,
    pub provider: String,
    pub model: String,
    pub resource: String,
    pub maximum_quota: SponsoredQuota,
    pub capability: InertGrantCapability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportSponsoredUsage {
    pub stable_request_id: String,
    pub request_fingerprint: String,
    pub actual_quota: Option<SponsoredQuota>,
    pub outcome: SponsoredUsageState,
}

pub struct SponsorshipService {
    pub(super) gate: SponsorshipGate,
    pub(super) store: Arc<dyn SponsorshipStore>,
    pub(super) provider: Arc<dyn SponsoredCredentialProvider>,
    pub(super) billing: Arc<dyn SponsoredBillingProvider>,
    pub(super) pricing: Arc<dyn SponsoredPricingProvider>,
    pub(super) clock: Arc<dyn SponsorshipClock>,
    pub(super) audit: Arc<SponsorshipAuditPseudonymizer>,
}

struct ActivationTransition {
    state: GrantActivationState,
    denial: Option<GrantActivationDenial>,
    receipt: Option<ProviderGrantReceipt>,
    capacity_held: bool,
    release_lease: bool,
}

impl SponsorshipService {
    pub fn new(
        gate: SponsorshipGate,
        store: Arc<dyn SponsorshipStore>,
        provider: Arc<dyn SponsoredCredentialProvider>,
        billing: Arc<dyn SponsoredBillingProvider>,
        pricing: Arc<dyn SponsoredPricingProvider>,
        clock: Arc<dyn SponsorshipClock>,
        audit: Arc<SponsorshipAuditPseudonymizer>,
    ) -> Self {
        Self {
            gate,
            store,
            provider,
            billing,
            pricing,
            clock,
            audit,
        }
    }

    pub fn gate(&self) -> &SponsorshipGate {
        &self.gate
    }

    /// Verify the immutable policy binding a publisher wants to place in a
    /// Trigger revision. This is an internal publication gate, not a public
    /// policy read: callers receive no owner, credential, usage, or provider
    /// metadata from it.
    pub async fn verify_publisher_policy_binding(
        &self,
        publisher: &SubjectId,
        binding: &SponsoredPolicyBinding,
    ) -> Result<(), SponsorshipError> {
        self.require_enabled().await?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let policy = self
            .store
            .policy(&binding.policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        if policy.owner.controller() != publisher || policy.tenant_id != binding.tenant_id {
            return Err(SponsorshipError::Forbidden);
        }
        if !policy.is_available_at(&now)
            || !self
                .gate
                .allowed_provider_policy_keys
                .contains(&policy.provider_policy.provider_policy_key)
        {
            return Err(SponsorshipError::PolicyUnavailable);
        }
        let credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or(SponsorshipError::CredentialUnavailable)?;
        credential.validate()?;
        if credential.record.owner != policy.owner
            || credential.record.state != SponsoredCredentialState::Active
            || credential.record.authority.provider != policy.provider_policy.provider
            || credential.record.authority.provider_policy_key
                != policy.provider_policy.provider_policy_key
        {
            return Err(SponsorshipError::CredentialUnavailable);
        }
        Ok(())
    }

    /// Read secret-free metadata for one owner-controlled credential. This
    /// remains available while the gate or kill switch blocks new spend so an
    /// owner can diagnose and complete revocation recovery.
    pub async fn credential_view(
        &self,
        caller: &Caller,
        credential_id: &SponsoredCredentialId,
    ) -> Result<SponsoredCredentialView, SponsorshipError> {
        let stored = self
            .store
            .credential(credential_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        stored.validate()?;
        require_sponsor_caller(caller, &stored.record.owner)?;
        Ok(SponsoredCredentialView::from(&stored.record))
    }

    /// Read one owner-controlled policy and its current durable accounting.
    pub async fn policy_view(
        &self,
        caller: &Caller,
        policy_id: &SponsorshipPolicyId,
    ) -> Result<SponsorshipPolicyView, SponsorshipError> {
        let policy = self
            .store
            .policy(policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        require_sponsor_caller(caller, &policy.owner)?;
        let usage = self.store.policy_usage(policy_id).await?;
        Ok(SponsorshipPolicyView::from_policy(&policy, usage))
    }

    /// Read one grant as either its contributor or sponsoring owner.
    pub async fn grant_view(
        &self,
        caller: &Caller,
        grant_id: &SponsoredGrantId,
    ) -> Result<SponsoredGrantView, SponsorshipError> {
        let grant = self
            .store
            .grant(grant_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        let policy = self
            .store
            .policy(&grant.policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        require_grant_control(caller, &policy.owner, &grant.contributor)?;
        Ok(SponsoredGrantView::from(&grant))
    }

    pub async fn grant_for_launch_view(
        &self,
        caller: &Caller,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrantView>, SponsorshipError> {
        let Some(grant) = self.store.grant_for_launch(launch_id).await? else {
            return Ok(None);
        };
        let policy = self
            .store
            .policy(&grant.policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        require_grant_control(caller, &policy.owner, &grant.contributor)?;
        Ok(Some(SponsoredGrantView::from(&grant)))
    }

    /// Read only the authenticated actor's pseudonymized audit stream.
    pub async fn actor_audit_events(
        &self,
        caller: &Caller,
        limit: usize,
    ) -> Result<Vec<SponsoredAuditEvent>, SponsorshipError> {
        if !caller
            .authorization
            .allows(Capability::ManageOwnSponsorships)
            && !caller.authorization.allows(Capability::UseOwnSessions)
        {
            return Err(SponsorshipError::Forbidden);
        }
        if limit == 0 || limit > MAX_SPONSORED_AUDIT_PAGE {
            return Err(TriggerValidationError::field(
                "limit",
                format!("must be between 1 and {MAX_SPONSORED_AUDIT_PAGE}"),
            )
            .into());
        }
        self.store
            .audit_events(&self.audit.actor_digest(&caller.subject), limit)
            .await
            .map_err(Into::into)
    }

    pub async fn engage_kill_switch(
        &self,
        caller: &Caller,
    ) -> Result<SponsorshipGateFence, SponsorshipError> {
        caller
            .authorization
            .require(Capability::ConfigurePlatform)
            .map_err(|_| SponsorshipError::Forbidden)?;
        let current = self.store.gate_fence().await?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::KillSwitchEngaged,
            &caller.subject,
            SponsoredAuditTarget::Platform,
            None,
            "engaged",
            now,
        );
        Ok(self.store.engage_kill_switch(current.epoch, audit).await?)
    }

    pub(super) async fn require_enabled(&self) -> Result<SponsorshipGateFence, SponsorshipError> {
        let fence = self.store.gate_fence().await?;
        self.gate.require_fence(fence)
    }

    pub async fn register_credential(
        &self,
        caller: &Caller,
        command: RegisterSponsoredCredential,
    ) -> Result<SponsoredCredentialView, SponsorshipError> {
        self.require_enabled().await?;
        validate_stable_request_id(&command.stable_request_id)?;
        require_sponsor_caller(caller, &command.owner)?;
        command.owner.validate()?;
        validate_policy_token("provider", &command.provider)?;
        validate_policy_token("provider_policy_key", &command.provider_policy_key)?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        if !self
            .gate
            .allowed_provider_policy_keys
            .contains(&command.provider_policy_key)
        {
            return Err(SponsorshipError::ProviderPolicyDenied);
        }
        let authority = self
            .provider
            .validate_ownership(
                &command.owner,
                &command.provider,
                &command.provider_policy_key,
                &command.credential,
            )
            .await
            .map_err(map_provider_error)?;
        if authority.provider != command.provider
            || authority.provider_policy_key != command.provider_policy_key
            || !authority.supports_expiry
            || !authority.supports_revocation
        {
            return Err(SponsorshipError::ProviderRejected);
        }
        let record = SponsoredCredentialRecord {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            id: deterministic_credential_id(&authority)?,
            owner: command.owner,
            authority,
            state: SponsoredCredentialState::Active,
            material_state: SponsoredCredentialMaterialState::Retained,
            generation: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
            revoked_at: None,
            pending_operation: None,
            superseded_operation: None,
            last_completed_operation: None,
        };
        record.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::CredentialRegistered,
            record.owner.controller(),
            SponsoredAuditTarget::Platform,
            Some(&command.stable_request_id),
            "registered",
            now,
        );
        let stored = self
            .store
            .insert_credential(
                &sensitive_digest(
                    "cma-sponsored-registration-idempotency-v1",
                    &command.stable_request_id,
                ),
                &record,
                &command.credential,
                audit,
            )
            .await?;
        Ok(SponsoredCredentialView::from(&stored))
    }

    pub async fn create_policy(
        &self,
        caller: &Caller,
        command: CreateSponsorshipPolicy,
    ) -> Result<SponsorshipPolicy, SponsorshipError> {
        self.require_enabled().await?;
        require_sponsor_caller(caller, &command.owner)?;
        let stored_credential = self
            .store
            .credential(&command.credential_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        stored_credential.validate()?;
        let credential = stored_credential.record;
        if command.owner != credential.owner {
            return Err(SponsorshipError::Forbidden);
        }
        if credential.state != SponsoredCredentialState::Active {
            return Err(SponsorshipError::CredentialUnavailable);
        }
        if credential.authority.provider != command.provider_policy.provider
            || credential.authority.provider_policy_key
                != command.provider_policy.provider_policy_key
            || !self
                .gate
                .allowed_provider_policy_keys
                .contains(&command.provider_policy.provider_policy_key)
        {
            return Err(SponsorshipError::ProviderPolicyDenied);
        }
        if command.aggregate_budget_micros > self.gate.maximum_policy_budget_micros
            || command.aggregate_quota.requests > self.gate.maximum_policy_requests
            || command.max_concurrent_launches > self.gate.maximum_concurrent_launches
        {
            return Err(TriggerValidationError::field(
                "sponsorship_policy",
                "exceeds a deployment-owned budget, request, or concurrency ceiling",
            )
            .into());
        }
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let policy = SponsorshipPolicy {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            id: SponsorshipPolicyId::new(),
            tenant_id: command.tenant_id,
            owner: command.owner,
            credential_id: command.credential_id,
            provider_policy: command.provider_policy,
            per_launch_quota: command.per_launch_quota,
            aggregate_quota: command.aggregate_quota,
            per_launch_budget_micros: command.per_launch_budget_micros,
            aggregate_budget_micros: command.aggregate_budget_micros,
            max_requests_per_minute: command.max_requests_per_minute,
            max_concurrent_launches: command.max_concurrent_launches,
            expires_at: command.expires_at,
            state: SponsorshipPolicyState::Active,
            generation: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
            revoked_at: None,
        };
        policy.validate()?;
        if !policy.is_available_at(&now) {
            return Err(SponsorshipError::PolicyUnavailable);
        }
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::PolicyCreated,
            policy.owner.controller(),
            SponsoredAuditTarget::Policy(policy.id.clone()),
            None,
            "created",
            now,
        );
        Ok(self.store.insert_policy(&policy, audit).await?)
    }

    pub async fn rotate_credential(
        &self,
        caller: &Caller,
        command: RotateSponsoredCredential,
    ) -> Result<SponsoredCredentialView, SponsorshipError> {
        self.require_enabled().await?;
        validate_stable_request_id(&command.stable_request_id)?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let stored_credential = self
            .store
            .credential(&command.credential_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        stored_credential.validate()?;
        require_sponsor_caller(caller, &stored_credential.record.owner)?;
        let replacement_digest = sensitive_digest(
            "cma-sponsored-credential-replacement-v1",
            command.replacement.expose(),
        );
        let client_request_id_digest = sensitive_digest(
            "cma-sponsored-credential-command-idempotency-v1",
            &command.stable_request_id,
        );
        if let Some(completed) = self
            .store
            .completed_credential_operation(
                &command.credential_id,
                ProviderOperationKind::RotateCredential,
                &client_request_id_digest,
            )
            .await?
        {
            if completed.payload_digest != replacement_digest {
                return Err(SponsorshipStoreError::Conflict.into());
            }
            return match completed.outcome {
                CompletedProviderOperationOutcome::Confirmed => {
                    Ok(SponsoredCredentialView::from(&stored_credential.record))
                }
                CompletedProviderOperationOutcome::Rejected => {
                    Err(SponsorshipError::ProviderRejected)
                }
            };
        }
        if stored_credential
            .record
            .last_completed_operation
            .as_ref()
            .is_some_and(|completed| {
                completed.kind == ProviderOperationKind::RotateCredential
                    && completed.outcome == CompletedProviderOperationOutcome::Confirmed
                    && completed.payload_digest == replacement_digest
            })
        {
            return Ok(SponsoredCredentialView::from(&stored_credential.record));
        }
        if stored_credential.record.material_state != SponsoredCredentialMaterialState::Retained {
            return Err(SponsorshipError::CredentialUnavailable);
        }
        let (mut credential, current_secret) = stored_credential.into_required_secret()?;
        if let Some(pending) = credential.pending_operation.as_ref().filter(|operation| {
            operation.kind == ProviderOperationKind::RotateCredential
                && operation.client_request_id_digest == client_request_id_digest
        }) {
            if pending.payload_digest != replacement_digest {
                return Err(SponsorshipStoreError::Conflict.into());
            }
        }
        if credential.state == SponsoredCredentialState::Active {
            let expected = credential.generation;
            let provider_request_id = credential_operation_id(
                ProviderOperationKind::RotateCredential,
                &credential.id,
                credential.generation,
                &replacement_digest,
            )?;
            credential.generation = credential.generation.checked_add(1).ok_or_else(|| {
                TriggerValidationError::field("credential.generation", "is exhausted")
            })?;
            credential.state = SponsoredCredentialState::RotationPending;
            credential.pending_operation = Some(PendingProviderOperation {
                kind: ProviderOperationKind::RotateCredential,
                phase: ProviderOperationPhase::Prepared,
                provider_request_id,
                client_request_id_digest: client_request_id_digest.clone(),
                payload_digest: replacement_digest.clone(),
                requested_at: now.clone(),
                attempted_at: None,
            });
            credential.updated_at = now.clone();
            credential.validate()?;
            let audit = SponsoredAuditEvent::privacy_safe(
                &self.audit,
                SponsoredAuditAction::CredentialRotated,
                &caller.subject,
                SponsoredAuditTarget::Platform,
                Some(&command.stable_request_id),
                "rotation_pending",
                now.clone(),
            );
            credential = self
                .store
                .stage_credential_rotation(expected, &credential, &command.replacement, audit)
                .await?;
        }
        if credential.state != SponsoredCredentialState::RotationPending
            || credential
                .pending_operation
                .as_ref()
                .is_none_or(|operation| {
                    operation.client_request_id_digest != client_request_id_digest
                        || operation.payload_digest != replacement_digest
                })
        {
            return Err(SponsorshipError::CredentialUnavailable);
        }
        let operation = credential.pending_operation.clone().ok_or_else(|| {
            SponsorshipStoreError::Malformed("rotation operation identity is missing".into())
        })?;
        let replacement = self
            .store
            .pending_credential_rotation_secret(&credential.id)
            .await?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed("pending rotation secret is missing".into())
            })?;
        let resolution = if operation.phase == ProviderOperationPhase::Prepared {
            let expected = credential.generation;
            credential.generation =
                next_generation(credential.generation, "credential.generation")?;
            let pending = credential.pending_operation.as_mut().ok_or_else(|| {
                SponsorshipStoreError::Malformed("rotation operation identity is missing".into())
            })?;
            pending.phase = ProviderOperationPhase::Attempting;
            pending.attempted_at = Some(now.clone());
            credential.updated_at = now.clone();
            credential.validate()?;
            let audit = SponsoredAuditEvent::privacy_safe(
                &self.audit,
                SponsoredAuditAction::CredentialRotated,
                &caller.subject,
                SponsoredAuditTarget::Platform,
                Some(&operation.provider_request_id),
                "rotation_attempting",
                now.clone(),
            );
            credential = self
                .store
                .replace_credential(expected, &credential, None, audit)
                .await?;
            match self
                .provider
                .rotate_credential(
                    &operation.provider_request_id,
                    &credential,
                    &current_secret,
                    &replacement,
                )
                .await
            {
                Ok(authority) => ProviderOperationResolution::Confirmed(authority),
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Unavailable) => {
                    self.persist_credential_operation_phase(
                        credential,
                        caller,
                        ProviderOperationPhase::Prepared,
                        None,
                        "rotation_deferred",
                        &now,
                    )
                    .await?;
                    return Err(SponsorshipError::ProviderUnavailable);
                }
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else if matches!(
            operation.phase,
            ProviderOperationPhase::Attempting | ProviderOperationPhase::Ambiguous
        ) {
            match self
                .provider
                .reconcile_credential_rotation(
                    &operation.provider_request_id,
                    &credential,
                    &current_secret,
                    &replacement,
                )
                .await
            {
                Ok(resolution) => resolution,
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::Unavailable)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else {
            return Err(SponsorshipError::ProviderRejected);
        };

        let authority = match resolution {
            ProviderOperationResolution::Confirmed(authority) => authority,
            ProviderOperationResolution::Pending => {
                self.persist_credential_operation_phase(
                    credential,
                    caller,
                    ProviderOperationPhase::Ambiguous,
                    Some(now.clone()),
                    "rotation_ambiguous",
                    &now,
                )
                .await?;
                return Err(SponsorshipError::ProviderAmbiguous);
            }
            ProviderOperationResolution::Rejected => {
                let expected = credential.generation;
                credential.generation =
                    next_generation(credential.generation, "credential.generation")?;
                credential.state = SponsoredCredentialState::Active;
                credential.pending_operation = None;
                credential.last_completed_operation = Some(CompletedProviderOperation {
                    kind: ProviderOperationKind::RotateCredential,
                    provider_request_id: operation.provider_request_id.clone(),
                    client_request_id_digest: operation.client_request_id_digest.clone(),
                    payload_digest: operation.payload_digest.clone(),
                    outcome: CompletedProviderOperationOutcome::Rejected,
                    completed_at: now.clone(),
                });
                credential.updated_at = now.clone();
                credential.validate()?;
                let audit = SponsoredAuditEvent::privacy_safe(
                    &self.audit,
                    SponsoredAuditAction::CredentialRotated,
                    &caller.subject,
                    SponsoredAuditTarget::Platform,
                    Some(&operation.provider_request_id),
                    "rotation_rejected",
                    now,
                );
                match self
                    .store
                    .abort_credential_rotation(expected, &credential, audit)
                    .await
                {
                    Ok(_) => return Err(SponsorshipError::ProviderRejected),
                    Err(
                        error @ (SponsorshipStoreError::Conflict
                        | SponsorshipStoreError::StaleGeneration),
                    ) => {
                        let latest = self
                            .store
                            .credential(&credential.id)
                            .await?
                            .ok_or(SponsorshipError::NotFound)?;
                        latest.validate()?;
                        if latest.record.state == SponsoredCredentialState::Revoked {
                            return Err(SponsorshipError::CredentialUnavailable);
                        }
                        return Err(error.into());
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        };
        if authority.provider != credential.authority.provider
            || authority.provider_policy_key != credential.authority.provider_policy_key
            || authority.state_version <= credential.authority.state_version
            || !authority.supports_expiry
            || !authority.supports_revocation
        {
            self.persist_credential_operation_phase(
                credential,
                caller,
                ProviderOperationPhase::Ambiguous,
                Some(now.clone()),
                "rotation_malformed",
                &now,
            )
            .await?;
            return Err(SponsorshipError::ProviderAmbiguous);
        }
        let expected = credential.generation;
        credential.generation = next_generation(credential.generation, "credential.generation")?;
        credential.authority = authority;
        credential.state = SponsoredCredentialState::Active;
        credential.pending_operation = None;
        credential.last_completed_operation = Some(CompletedProviderOperation {
            kind: ProviderOperationKind::RotateCredential,
            provider_request_id: operation.provider_request_id.clone(),
            client_request_id_digest: operation.client_request_id_digest.clone(),
            payload_digest: operation.payload_digest.clone(),
            outcome: CompletedProviderOperationOutcome::Confirmed,
            completed_at: now.clone(),
        });
        credential.updated_at = now.clone();
        credential.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::CredentialRotated,
            &caller.subject,
            SponsoredAuditTarget::Platform,
            Some(&operation.provider_request_id),
            "rotated",
            now.clone(),
        );
        let completion = self
            .store
            .complete_credential_rotation(expected, &credential, audit)
            .await;
        match completion {
            Ok(stored) => Ok(SponsoredCredentialView::from(&stored)),
            Err(
                error @ (SponsorshipStoreError::Conflict | SponsorshipStoreError::StaleGeneration),
            ) => {
                let latest = self
                    .store
                    .credential(&credential.id)
                    .await?
                    .ok_or(SponsorshipError::NotFound)?;
                latest.validate()?;
                if latest.record.state == SponsoredCredentialState::Revoked {
                    return Err(SponsorshipError::CredentialUnavailable);
                }
                if latest.record.state == SponsoredCredentialState::Active
                    && latest
                        .record
                        .last_completed_operation
                        .as_ref()
                        .is_some_and(|completed| {
                            completed.kind == ProviderOperationKind::RotateCredential
                                && completed.provider_request_id == operation.provider_request_id
                                && completed.outcome == CompletedProviderOperationOutcome::Confirmed
                        })
                {
                    return Ok(SponsoredCredentialView::from(&latest.record));
                }
                if latest.record.state == SponsoredCredentialState::RotationPending
                    && latest
                        .record
                        .pending_operation
                        .as_ref()
                        .is_some_and(|pending| {
                            pending.provider_request_id == operation.provider_request_id
                                && pending.payload_digest == operation.payload_digest
                        })
                {
                    if latest
                        .record
                        .pending_operation
                        .as_ref()
                        .is_some_and(|pending| pending.phase != ProviderOperationPhase::Ambiguous)
                    {
                        self.persist_credential_operation_phase(
                            latest.record,
                            caller,
                            ProviderOperationPhase::Ambiguous,
                            Some(now.clone()),
                            "rotation_commit_ambiguous",
                            &now,
                        )
                        .await?;
                    }
                    return Err(SponsorshipError::ProviderAmbiguous);
                }
                if latest.record.state == SponsoredCredentialState::RevocationPending {
                    return Err(SponsorshipError::ProviderAmbiguous);
                }
                Err(error.into())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Persist a fail-closed state before asking the provider to retire the
    /// credential. Retrying a pending record reuses the stable provider request.
    pub async fn revoke_credential(
        &self,
        caller: &Caller,
        credential_id: &SponsoredCredentialId,
        stable_request_id: &str,
    ) -> Result<SponsoredCredentialView, SponsorshipError> {
        validate_stable_request_id(stable_request_id)?;
        let client_request_id_digest = sensitive_digest(
            "cma-sponsored-credential-command-idempotency-v1",
            stable_request_id,
        );
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let stored_credential = self
            .store
            .credential(credential_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        stored_credential.validate()?;
        require_sponsor_caller(caller, &stored_credential.record.owner)?;
        if stored_credential.record.state == SponsoredCredentialState::Revoked {
            return Ok(SponsoredCredentialView::from(&stored_credential.record));
        }
        let (mut credential, secret) = stored_credential.into_required_secret()?;
        if matches!(
            credential.state,
            SponsoredCredentialState::Active | SponsoredCredentialState::RotationPending
        ) {
            let expected = credential.generation;
            let superseded_rotation = (credential.state
                == SponsoredCredentialState::RotationPending)
                .then(|| credential.pending_operation.clone())
                .flatten();
            let payload_digest = canonical_digest(&(
                "cma-sponsored-credential-revocation-payload-v1",
                &credential.authority.provider_credential_id_digest,
                superseded_rotation
                    .as_ref()
                    .map(|operation| (&operation.provider_request_id, &operation.payload_digest)),
            ))?;
            credential.generation =
                next_generation(credential.generation, "credential.generation")?;
            credential.state = SponsoredCredentialState::RevocationPending;
            credential.pending_operation = Some(PendingProviderOperation {
                provider_request_id: credential_operation_id(
                    ProviderOperationKind::RevokeCredential,
                    &credential.id,
                    expected,
                    &payload_digest,
                )?,
                kind: ProviderOperationKind::RevokeCredential,
                phase: ProviderOperationPhase::Prepared,
                client_request_id_digest: client_request_id_digest.clone(),
                payload_digest,
                requested_at: now.clone(),
                attempted_at: None,
            });
            credential.superseded_operation = superseded_rotation;
            credential.updated_at = now.clone();
            credential.validate()?;
            let audit = SponsoredAuditEvent::privacy_safe(
                &self.audit,
                SponsoredAuditAction::CredentialRevocationRequested,
                &caller.subject,
                SponsoredAuditTarget::Platform,
                Some(stable_request_id),
                "revocation_pending",
                now.clone(),
            );
            credential = self
                .store
                .replace_credential(expected, &credential, None, audit)
                .await?;
        }
        let staged_replacement = if credential.superseded_operation.is_some() {
            Some(
                self.store
                    .pending_credential_rotation_secret(&credential.id)
                    .await?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "superseded rotation has no staged replacement secret".into(),
                        )
                    })?,
            )
        } else {
            None
        };
        let operation = credential
            .pending_operation
            .as_ref()
            .filter(|_| credential.state == SponsoredCredentialState::RevocationPending)
            .cloned()
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed("revocation operation identity is missing".into())
            })?;
        let resolution = if operation.phase == ProviderOperationPhase::Prepared {
            let expected = credential.generation;
            credential.generation =
                next_generation(credential.generation, "credential.generation")?;
            let pending = credential.pending_operation.as_mut().ok_or_else(|| {
                SponsorshipStoreError::Malformed("revocation operation identity is missing".into())
            })?;
            pending.phase = ProviderOperationPhase::Attempting;
            pending.attempted_at = Some(now.clone());
            credential.updated_at = now.clone();
            credential.validate()?;
            let audit = SponsoredAuditEvent::privacy_safe(
                &self.audit,
                SponsoredAuditAction::CredentialRevocationRequested,
                &caller.subject,
                SponsoredAuditTarget::Platform,
                Some(&operation.provider_request_id),
                "revocation_attempting",
                now.clone(),
            );
            credential = self
                .store
                .replace_credential(expected, &credential, None, audit)
                .await?;
            match self
                .provider
                .revoke_credential(
                    &operation.provider_request_id,
                    &credential,
                    &secret,
                    staged_replacement.as_ref(),
                    credential.superseded_operation.as_ref(),
                )
                .await
            {
                Ok(()) => ProviderOperationResolution::Confirmed(()),
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Unavailable) => {
                    self.persist_credential_operation_phase(
                        credential,
                        caller,
                        ProviderOperationPhase::Prepared,
                        None,
                        "revocation_deferred",
                        &now,
                    )
                    .await?;
                    return Err(SponsorshipError::ProviderUnavailable);
                }
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else if matches!(
            operation.phase,
            ProviderOperationPhase::Attempting | ProviderOperationPhase::Ambiguous
        ) {
            match self
                .provider
                .reconcile_credential_revocation(
                    &operation.provider_request_id,
                    &credential,
                    &secret,
                    staged_replacement.as_ref(),
                    credential.superseded_operation.as_ref(),
                )
                .await
            {
                Ok(resolution) => resolution,
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::Unavailable)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else {
            return Err(SponsorshipError::ProviderRejected);
        };
        match resolution {
            ProviderOperationResolution::Confirmed(()) => {}
            ProviderOperationResolution::Pending => {
                self.persist_credential_operation_phase(
                    credential,
                    caller,
                    ProviderOperationPhase::Ambiguous,
                    Some(now.clone()),
                    "revocation_ambiguous",
                    &now,
                )
                .await?;
                return Err(SponsorshipError::ProviderAmbiguous);
            }
            ProviderOperationResolution::Rejected => {
                // Keep the credential fail-closed and retain material solely
                // for the explicit owner recovery workflow tracked by #675.
                self.persist_credential_operation_phase(
                    credential,
                    caller,
                    ProviderOperationPhase::Rejected,
                    Some(now.clone()),
                    "revocation_rejected",
                    &now,
                )
                .await?;
                return Err(SponsorshipError::ProviderRejected);
            }
        }
        let expected = credential.generation;
        credential.generation = next_generation(credential.generation, "credential.generation")?;
        credential.state = SponsoredCredentialState::Revoked;
        credential.material_state = SponsoredCredentialMaterialState::Retired;
        credential.pending_operation = None;
        credential.superseded_operation = None;
        credential.last_completed_operation = Some(CompletedProviderOperation {
            kind: ProviderOperationKind::RevokeCredential,
            provider_request_id: operation.provider_request_id.clone(),
            client_request_id_digest: operation.client_request_id_digest.clone(),
            payload_digest: operation.payload_digest.clone(),
            outcome: CompletedProviderOperationOutcome::Confirmed,
            completed_at: now.clone(),
        });
        credential.revoked_at = Some(now.clone());
        credential.updated_at = now.clone();
        credential.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::CredentialRevoked,
            &caller.subject,
            SponsoredAuditTarget::Platform,
            Some(&operation.provider_request_id),
            "revoked",
            now,
        );
        let stored = self
            .store
            .complete_credential_revocation(expected, &credential, audit)
            .await?;
        Ok(SponsoredCredentialView::from(&stored))
    }

    pub async fn suspend_policy(
        &self,
        caller: &Caller,
        policy_id: &SponsorshipPolicyId,
    ) -> Result<SponsorshipPolicy, SponsorshipError> {
        self.change_policy_state(caller, policy_id, SponsorshipPolicyState::Suspended)
            .await
    }

    pub async fn revoke_policy(
        &self,
        caller: &Caller,
        policy_id: &SponsorshipPolicyId,
    ) -> Result<SponsorshipPolicy, SponsorshipError> {
        self.change_policy_state(caller, policy_id, SponsorshipPolicyState::Revoked)
            .await
    }

    async fn change_policy_state(
        &self,
        caller: &Caller,
        policy_id: &SponsorshipPolicyId,
        state: SponsorshipPolicyState,
    ) -> Result<SponsorshipPolicy, SponsorshipError> {
        loop {
            let mut policy = self
                .store
                .policy(policy_id)
                .await?
                .ok_or(SponsorshipError::NotFound)?;
            require_sponsor_caller(caller, &policy.owner)?;
            if policy.state == SponsorshipPolicyState::Revoked || policy.state == state {
                return Ok(policy);
            }
            let now = self.clock.now();
            validate_timestamp("now", &now)?;
            let expected = policy.generation;
            policy.generation = policy.generation.checked_add(1).ok_or_else(|| {
                TriggerValidationError::field("sponsorship_policy.generation", "is exhausted")
            })?;
            policy.state = state;
            policy.updated_at = now.clone();
            policy.revoked_at = (state == SponsorshipPolicyState::Revoked).then(|| now.clone());
            policy.validate()?;
            let action = match state {
                SponsorshipPolicyState::Suspended => SponsoredAuditAction::PolicySuspended,
                SponsorshipPolicyState::Revoked => SponsoredAuditAction::PolicyRevoked,
                SponsorshipPolicyState::Active => {
                    return Err(TriggerValidationError::field(
                        "sponsorship_policy.state",
                        "reactivation requires a new reviewed policy generation",
                    )
                    .into())
                }
            };
            let audit = SponsoredAuditEvent::privacy_safe(
                &self.audit,
                action,
                &caller.subject,
                SponsoredAuditTarget::Policy(policy.id.clone()),
                None,
                if state == SponsorshipPolicyState::Revoked {
                    "revoked"
                } else {
                    "suspended"
                },
                now,
            );
            match self.store.replace_policy(expected, &policy, audit).await {
                Ok(stored) => return Ok(stored),
                Err(SponsorshipStoreError::StaleGeneration) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub async fn terms(
        &self,
        caller: &Caller,
        launch_id: &LaunchId,
    ) -> Result<SponsoredGrantTerms, SponsorshipError> {
        Ok(self.terms_review(caller, launch_id).await?.terms)
    }

    pub async fn terms_review(
        &self,
        caller: &Caller,
        launch_id: &LaunchId,
    ) -> Result<SponsoredTermsReview, SponsorshipError> {
        self.require_enabled().await?;
        require_contributor_capability(caller)?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let launch = self
            .store
            .launch_authority(launch_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        launch.validate()?;
        launch.require_terms_review()?;
        if launch.contributor != caller.subject {
            return Err(SponsorshipError::Forbidden);
        }
        let policy = self
            .store
            .policy(&launch.policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        if !policy.is_available_at(&now) || policy.tenant_id != launch.tenant_id {
            return Err(SponsorshipError::PolicyUnavailable);
        }
        let usage = self.store.policy_usage(&policy.id).await?;
        let terms = SponsoredGrantTerms::from_policy(&policy, &usage)?;
        Ok(SponsoredTermsReview {
            terms,
            launch_snapshot_digest: launch.launch_snapshot_digest,
        })
    }

    pub async fn accept_terms(
        &self,
        caller: &Caller,
        command: AcceptSponsoredTerms,
    ) -> Result<SponsoredGrantAcceptance, SponsorshipError> {
        validate_stable_request_id(&command.idempotency_key)?;
        require_contributor_capability(caller)?;
        let idempotency_key_hash = sensitive_digest(
            "cma-sponsored-acceptance-idempotency-v1",
            &command.idempotency_key,
        );
        if let Some(stored) = self.store.acceptance_for_launch(&command.launch_id).await? {
            if stored.contributor != caller.subject {
                return Err(SponsorshipError::Forbidden);
            }
            if stored.idempotency_key_hash == idempotency_key_hash {
                if stored.policy_id == command.policy_id
                    && stored.terms_digest == command.terms_digest
                    && stored.launch_snapshot_digest == command.launch_snapshot_digest
                {
                    stored.validate()?;
                    return Ok(stored);
                }
                return Err(SponsorshipStoreError::Conflict.into());
            }
        }
        let gate_fence = self.require_enabled().await?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let launch = self
            .store
            .launch_authority(&command.launch_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        launch.validate()?;
        launch.require_terms_review()?;
        if launch.contributor != caller.subject
            || launch.policy_id != command.policy_id
            || launch.launch_snapshot_digest != command.launch_snapshot_digest
        {
            return Err(SponsorshipError::Forbidden);
        }
        let policy = self
            .store
            .policy(&command.policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        if !policy.is_available_at(&now) || policy.tenant_id != launch.tenant_id {
            return Err(SponsorshipError::PolicyUnavailable);
        }
        let usage = self.store.policy_usage(&policy.id).await?;
        let terms = SponsoredGrantTerms::from_policy(&policy, &usage)?;
        if terms.digest != command.terms_digest {
            return Err(SponsorshipError::TermsChanged);
        }
        let mut acceptance = SponsoredGrantAcceptance {
            launch_id: command.launch_id,
            contributor: caller.subject.clone(),
            revision_id: launch.revision_id.clone(),
            revision_digest: launch.revision_digest.clone(),
            policy_id: command.policy_id,
            policy_generation: terms.policy_generation,
            terms_digest: terms.digest,
            launch_snapshot_digest: command.launch_snapshot_digest,
            accepted_at: now.clone(),
            idempotency_key_hash,
            digest: String::new(),
        };
        acceptance.digest = acceptance.calculate_digest()?;
        acceptance.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::TermsAccepted,
            &acceptance.contributor,
            SponsoredAuditTarget::Policy(acceptance.policy_id.clone()),
            None,
            "accepted",
            now,
        );
        Ok(self
            .store
            .insert_acceptance(PersistSponsoredAcceptance {
                acceptance,
                launch_authority: launch.clone(),
                gate_fence,
                audit,
            })
            .await?)
    }

    pub async fn activate_grant(
        &self,
        caller: &Caller,
        command: ActivateSponsoredGrant,
    ) -> Result<SponsoredGrant, SponsorshipError> {
        let gate_fence = self.require_enabled().await?;
        require_contributor_capability(caller)?;
        validate_stable_request_id(&command.stable_request_id)?;
        let mut now = self.clock.now();
        validate_timestamp("now", &now)?;
        let launch = self
            .store
            .launch_authority(&command.launch_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        launch.validate()?;
        let session_id = launch.require_activation()?.clone();
        if launch.contributor != caller.subject {
            return Err(SponsorshipError::Forbidden);
        }
        if let Some(existing) = self.store.grant_for_launch(&command.launch_id).await? {
            require_grant_bindings(&existing, &launch)?;
            return Ok(existing);
        }
        let policy = self
            .store
            .policy(&launch.policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        if !policy.is_available_at(&now) || policy.tenant_id != launch.tenant_id {
            return Err(SponsorshipError::PolicyUnavailable);
        }
        let acceptance = self
            .store
            .acceptance_for_launch(&command.launch_id)
            .await?
            .ok_or(SponsorshipError::TermsNotAccepted)?;
        require_acceptance_launch_bindings(&acceptance, &launch)?;
        let stored_credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or(SponsorshipError::CredentialUnavailable)?;
        stored_credential.validate()?;
        if stored_credential.record.state != SponsoredCredentialState::Active
            || stored_credential.record.owner != policy.owner
            || stored_credential.record.authority.provider != policy.provider_policy.provider
            || stored_credential.record.authority.provider_policy_key
                != policy.provider_policy.provider_policy_key
        {
            return Err(SponsorshipError::CredentialUnavailable);
        }
        let (credential, secret) = stored_credential.into_required_secret()?;
        let allocation = SponsoredUsageSummary {
            quota: policy.per_launch_quota,
            spend_micros: policy.per_launch_budget_micros,
        };
        let holder = sensitive_digest(
            "cma-sponsored-activation-holder-v1",
            &command.stable_request_id,
        );
        let billing_request_id = activation_operation_id(
            "billing",
            &launch,
            &acceptance,
            policy.generation,
            credential.generation,
        )?;
        let provider_request_id = activation_operation_id(
            "provider",
            &launch,
            &acceptance,
            policy.generation,
            credential.generation,
        )?;
        let request_fingerprint = activation_fingerprint(
            &launch,
            &acceptance,
            &gate_fence,
            &allocation,
            policy.generation,
            credential.generation,
        )?;
        let claim = self
            .store
            .claim_grant_activation(ClaimGrantActivation {
                candidate: GrantActivationIntent {
                    launch_id: command.launch_id.clone(),
                    session_id: session_id.clone(),
                    policy_id: policy.id.clone(),
                    policy_generation: policy.generation,
                    credential_generation: credential.generation,
                    launch_authority_digest: launch.digest.clone(),
                    acceptance_digest: acceptance.digest.clone(),
                    gate_fence: gate_fence.clone(),
                    allocation: allocation.clone(),
                    request_fingerprint,
                    billing_request_id,
                    provider_request_id: provider_request_id.clone(),
                    state: GrantActivationState::AwaitingBilling,
                    denial: None,
                    capacity_held: false,
                    revision: 1,
                    lease: None,
                    provider_receipt: None,
                    cleanup_operation: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
                holder: holder.clone(),
                launch_authority: launch.clone(),
                acceptance: acceptance.clone(),
            })
            .await?;
        if !claim.acquired {
            if claim.intent.state == GrantActivationState::Committed {
                return self
                    .store
                    .grant_for_launch(&command.launch_id)
                    .await?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed("committed activation has no grant".into())
                            .into()
                    });
            }
            if let Some(denial) = claim.intent.denial {
                return Err(match denial {
                    GrantActivationDenial::BillingRejected => SponsorshipError::BillingRejected,
                    GrantActivationDenial::ProviderRejected => SponsorshipError::ProviderRejected,
                    GrantActivationDenial::AuthorityInvalidated => SponsorshipError::ScopeDenied,
                });
            }
            return Err(SponsorshipError::ActivationInProgress);
        }
        let mut intent = claim.intent;
        intent.validate()?;

        let mut provider_dispatch_fenced = false;
        if matches!(intent.state, GrantActivationState::AwaitingBilling) {
            intent = self
                .advance_activation(
                    intent,
                    &holder,
                    ActivationTransition {
                        state: GrantActivationState::BillingAttempting,
                        denial: None,
                        receipt: None,
                        capacity_held: true,
                        release_lease: false,
                    },
                    &now,
                )
                .await?;
            let billing_result = self
                .billing
                .confirm_grant(&intent.billing_request_id, &policy, &command.launch_id)
                .await;
            now = self.clock.now();
            validate_timestamp("now", &now)?;
            let resolution = match billing_result {
                Ok(result) => ProviderOperationResolution::Confirmed(result),
                Err(SponsoredBillingError::Ambiguous) => ProviderOperationResolution::Pending,
                Err(SponsoredBillingError::Unavailable) => {
                    self.advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::AwaitingBilling,
                            denial: None,
                            receipt: None,
                            capacity_held: false,
                            release_lease: true,
                        },
                        &now,
                    )
                    .await?;
                    return Err(SponsorshipError::BillingUnavailable);
                }
            };
            (intent, provider_dispatch_fenced) = self
                .apply_billing_resolution(intent, &holder, resolution, &launch, &acceptance, &now)
                .await?;
        } else if matches!(
            intent.state,
            GrantActivationState::BillingAttempting | GrantActivationState::BillingAmbiguous
        ) {
            let billing_result = self
                .billing
                .reconcile_grant(&intent.billing_request_id, &policy, &command.launch_id)
                .await;
            now = self.clock.now();
            validate_timestamp("now", &now)?;
            let resolution = match billing_result {
                Ok(resolution) => resolution,
                Err(SponsoredBillingError::Ambiguous) => ProviderOperationResolution::Pending,
                Err(SponsoredBillingError::Unavailable) => {
                    self.advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::BillingAmbiguous,
                            denial: None,
                            receipt: None,
                            capacity_held: true,
                            release_lease: true,
                        },
                        &now,
                    )
                    .await?;
                    return Err(SponsorshipError::BillingUnavailable);
                }
            };
            (intent, provider_dispatch_fenced) = self
                .apply_billing_resolution(intent, &holder, resolution, &launch, &acceptance, &now)
                .await?;
        }

        if intent.state == GrantActivationState::AwaitingProvider {
            intent = self
                .begin_provider_grant_issue(intent, &holder, &launch, &acceptance, &now)
                .await?;
            provider_dispatch_fenced = true;
        }

        let provider_resolution = if provider_dispatch_fenced {
            let provider_result = self
                .provider
                .issue_grant(
                    &intent.provider_request_id,
                    &credential,
                    &secret,
                    &policy,
                    &command.launch_id,
                    &session_id,
                )
                .await;
            now = self.clock.now();
            validate_timestamp("now", &now)?;
            match provider_result {
                Ok(receipt) => ProviderOperationResolution::Confirmed(receipt),
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Unavailable) => {
                    self.advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::AwaitingProvider,
                            denial: None,
                            receipt: None,
                            capacity_held: false,
                            release_lease: true,
                        },
                        &now,
                    )
                    .await?;
                    return Err(SponsorshipError::ProviderUnavailable);
                }
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else {
            match intent.state {
                GrantActivationState::ProviderConfirmed => ProviderOperationResolution::Confirmed(
                    intent.provider_receipt.clone().ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "provider-confirmed activation has no retained receipt".into(),
                        )
                    })?,
                ),
                GrantActivationState::ProviderAttempting
                | GrantActivationState::ProviderAmbiguous => {
                    let provider_result = self
                        .provider
                        .reconcile_grant_issue(
                            &intent.provider_request_id,
                            &credential,
                            &secret,
                            &policy,
                            &command.launch_id,
                            &session_id,
                        )
                        .await;
                    now = self.clock.now();
                    validate_timestamp("now", &now)?;
                    match provider_result {
                        Ok(resolution) => resolution,
                        Err(SponsoredProviderError::Rejected) => {
                            ProviderOperationResolution::Rejected
                        }
                        Err(SponsoredProviderError::Ambiguous)
                        | Err(SponsoredProviderError::Unavailable)
                        | Err(SponsoredProviderError::MalformedEvidence) => {
                            ProviderOperationResolution::Pending
                        }
                    }
                }
                _ => {
                    return Err(SponsorshipStoreError::Malformed(
                        "activation reached provider work in an invalid state".into(),
                    )
                    .into())
                }
            }
        };

        let receipt = match provider_resolution {
            ProviderOperationResolution::Confirmed(receipt) => {
                if validate_provider_grant_receipt(&receipt, &policy, &now).is_err() {
                    self.advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::ProviderAmbiguous,
                            denial: None,
                            receipt: Some(receipt),
                            capacity_held: true,
                            release_lease: true,
                        },
                        &now,
                    )
                    .await?;
                    return Err(SponsorshipError::ProviderAmbiguous);
                }
                intent = self
                    .advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::ProviderConfirmed,
                            denial: None,
                            receipt: Some(receipt.clone()),
                            capacity_held: true,
                            release_lease: false,
                        },
                        &now,
                    )
                    .await?;
                receipt
            }
            ProviderOperationResolution::Pending => {
                self.advance_activation(
                    intent,
                    &holder,
                    ActivationTransition {
                        state: GrantActivationState::ProviderAmbiguous,
                        denial: None,
                        receipt: None,
                        capacity_held: true,
                        release_lease: true,
                    },
                    &now,
                )
                .await?;
                return Err(SponsorshipError::ProviderAmbiguous);
            }
            ProviderOperationResolution::Rejected => {
                self.advance_activation(
                    intent,
                    &holder,
                    ActivationTransition {
                        state: GrantActivationState::Denied,
                        denial: Some(GrantActivationDenial::ProviderRejected),
                        receipt: None,
                        capacity_held: false,
                        release_lease: true,
                    },
                    &now,
                )
                .await?;
                return Err(SponsorshipError::ProviderRejected);
            }
        };
        let id = deterministic_grant_id(&provider_request_id)?;
        let mut grant = SponsoredGrant {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            id: id.clone(),
            tenant_id: launch.tenant_id.clone(),
            policy_id: policy.id.clone(),
            policy_generation: policy.generation,
            credential_generation: credential.generation,
            trigger_revision_id: launch.revision_id.clone(),
            trigger_revision_digest: launch.revision_digest.clone(),
            launch_id: command.launch_id.clone(),
            session_id: session_id.clone(),
            contributor: caller.subject.clone(),
            terms_digest: acceptance.terms_digest.clone(),
            acceptance: acceptance.clone(),
            authority: ProviderGrantAuthority {
                provider_grant_id_digest: sensitive_digest(
                    "cma-provider-grant-id-v1",
                    receipt
                        .provider_grant_id
                        .expose_for_provider_or_encryption(),
                ),
                provider_state_version: receipt.provider_state_version,
                expires_at: receipt.expires_at.clone(),
                revocation_handle_digest: sensitive_digest(
                    "cma-provider-revocation-handle-v1",
                    receipt
                        .revocation_handle
                        .expose_for_provider_or_encryption(),
                ),
                issued_at: receipt.issued_at.clone(),
            },
            capability: InertGrantCapability {
                grant_id: id,
                grant_generation: 1,
                policy_generation: policy.generation,
                launch_id: command.launch_id.clone(),
                session_id,
                expires_at: receipt.expires_at.clone(),
                fingerprint: String::new(),
            },
            state: SponsoredGrantState::Active,
            allocation,
            usage: SponsoredUsageSummary::default(),
            generation: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
            revoked_at: None,
            pending_revocation: None,
        };
        grant.refresh_capability()?;
        grant.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::GrantIssued,
            &caller.subject,
            SponsoredAuditTarget::Grant {
                policy_id: policy.id.clone(),
                grant_id: grant.id.clone(),
            },
            Some(&provider_request_id),
            "issued",
            now.clone(),
        );
        let expected_revision = intent.revision;
        intent.revision = next_generation(intent.revision, "grant_activation.revision")?;
        intent.state = GrantActivationState::Committed;
        intent.capacity_held = false;
        intent.lease = None;
        intent.denial = None;
        intent.updated_at = grant.updated_at.clone();
        intent.validate()?;
        let result = self
            .store
            .commit_grant_activation(CommitGrantActivation {
                expected_revision,
                holder: holder.clone(),
                intent,
                grant,
                launch_authority: launch.clone(),
                acceptance,
                gate_fence,
                now: now.clone(),
                audit,
            })
            .await;
        match result {
            Ok(grant) => Ok(grant),
            Err(commit_error) => {
                if let Some(grant) = self.store.grant_for_launch(&command.launch_id).await? {
                    require_grant_bindings(&grant, &launch)?;
                    return Ok(grant);
                }
                if matches!(
                    &commit_error,
                    SponsorshipStoreError::GateChanged
                        | SponsorshipStoreError::LaunchAuthorityChanged
                        | SponsorshipStoreError::AcceptanceChanged
                        | SponsorshipStoreError::GrantUnavailable
                ) {
                    let durable = self
                        .store
                        .grant_activation(&command.launch_id)
                        .await?
                        .ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "failed activation commit has no durable intent".into(),
                            )
                        })?;
                    if durable.state == GrantActivationState::ProviderConfirmed {
                        self.cleanup_uncommitted_activation(&caller.subject, durable, holder, &now)
                            .await?;
                    }
                }
                Err(commit_error.into())
            }
        }
    }

    /// Reconcile and revoke provider authority retained by an activation whose
    /// final store-side authority CAS could not commit. This remediation stays
    /// available after the kill switch or Launch transition that caused the
    /// failure; it cannot create a grant and releases capacity only after
    /// provider read-back confirms revocation.
    pub async fn reconcile_activation_cleanup(
        &self,
        caller: &Caller,
        launch_id: &LaunchId,
    ) -> Result<(), SponsorshipError> {
        if !caller
            .authorization
            .allows(Capability::ReconcileSponsoredUsage)
            && !caller.authorization.allows(Capability::ConfigurePlatform)
        {
            return Err(SponsorshipError::Forbidden);
        }
        if self.store.grant_for_launch(launch_id).await?.is_some() {
            return Ok(());
        }
        let intent = self
            .store
            .grant_activation(launch_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        if matches!(
            intent.state,
            GrantActivationState::Cleaned | GrantActivationState::Denied
        ) {
            return Ok(());
        }
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let holder = sensitive_digest(
            "cma-sponsored-activation-cleanup-holder-v1",
            &intent.provider_request_id,
        );
        if matches!(
            intent.state,
            GrantActivationState::BillingAttempting
                | GrantActivationState::BillingAmbiguous
                | GrantActivationState::ProviderAttempting
        ) || (intent.state == GrantActivationState::ProviderAmbiguous
            && intent.provider_receipt.is_none())
        {
            return self
                .recover_invalidated_activation(&caller.subject, intent, holder, &now)
                .await;
        }
        if matches!(
            intent.state,
            GrantActivationState::ProviderConfirmed
                | GrantActivationState::ProviderAmbiguous
                | GrantActivationState::CleanupPending
        ) && intent.provider_receipt.is_some()
        {
            return self
                .cleanup_uncommitted_activation(&caller.subject, intent, holder, &now)
                .await;
        }
        Err(SponsorshipError::ActivationInProgress)
    }

    async fn recover_invalidated_activation(
        &self,
        actor: &SubjectId,
        intent: GrantActivationIntent,
        holder: String,
        now: &str,
    ) -> Result<(), SponsorshipError> {
        let claim = self
            .store
            .claim_invalidated_grant_activation(ClaimInvalidatedGrantActivation {
                launch_id: intent.launch_id.clone(),
                holder: holder.clone(),
            })
            .await?;
        if !claim.acquired {
            return Err(SponsorshipError::ActivationInProgress);
        }
        let mut intent = claim.intent;
        let policy = self.store.policy(&intent.policy_id).await?.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "invalidated activation has no sponsorship policy".into(),
            )
        })?;
        if matches!(
            intent.state,
            GrantActivationState::BillingAttempting | GrantActivationState::BillingAmbiguous
        ) {
            let resolution = match self
                .billing
                .reconcile_grant(&intent.billing_request_id, &policy, &intent.launch_id)
                .await
            {
                Ok(resolution) => resolution,
                Err(SponsoredBillingError::Ambiguous) => ProviderOperationResolution::Pending,
                Err(SponsoredBillingError::Unavailable) => {
                    self.advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::BillingAmbiguous,
                            denial: None,
                            receipt: None,
                            capacity_held: true,
                            release_lease: true,
                        },
                        now,
                    )
                    .await?;
                    return Err(SponsorshipError::BillingUnavailable);
                }
            };
            return match resolution {
                ProviderOperationResolution::Pending => {
                    self.advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::BillingAmbiguous,
                            denial: None,
                            receipt: None,
                            capacity_held: true,
                            release_lease: true,
                        },
                        now,
                    )
                    .await?;
                    Err(SponsorshipError::BillingAmbiguous)
                }
                ProviderOperationResolution::Confirmed(_)
                | ProviderOperationResolution::Rejected => {
                    self.complete_invalidated_activation(actor, intent, holder, now)
                        .await
                }
            };
        }

        let stored_credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "invalidated activation has no provider credential".into(),
                )
            })?;
        stored_credential.validate()?;
        if stored_credential.record.material_state == SponsoredCredentialMaterialState::Retired {
            return self
                .complete_invalidated_activation(actor, intent, holder, now)
                .await;
        }
        let (credential, secret) = stored_credential.into_required_secret()?;
        let resolution = match self
            .provider
            .reconcile_grant_issue(
                &intent.provider_request_id,
                &credential,
                &secret,
                &policy,
                &intent.launch_id,
                &intent.session_id,
            )
            .await
        {
            Ok(resolution) => resolution,
            Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
            Err(SponsoredProviderError::Ambiguous)
            | Err(SponsoredProviderError::Unavailable)
            | Err(SponsoredProviderError::MalformedEvidence) => {
                ProviderOperationResolution::Pending
            }
        };
        match resolution {
            ProviderOperationResolution::Confirmed(receipt) => {
                intent = self
                    .advance_activation(
                        intent,
                        &holder,
                        ActivationTransition {
                            state: GrantActivationState::ProviderConfirmed,
                            denial: None,
                            receipt: Some(receipt),
                            capacity_held: true,
                            release_lease: false,
                        },
                        now,
                    )
                    .await?;
                self.cleanup_uncommitted_activation(actor, intent, holder, now)
                    .await
            }
            ProviderOperationResolution::Pending => {
                self.advance_activation(
                    intent,
                    &holder,
                    ActivationTransition {
                        state: GrantActivationState::ProviderAmbiguous,
                        denial: None,
                        receipt: None,
                        capacity_held: true,
                        release_lease: true,
                    },
                    now,
                )
                .await?;
                Err(SponsorshipError::ProviderAmbiguous)
            }
            ProviderOperationResolution::Rejected => {
                self.complete_invalidated_activation(actor, intent, holder, now)
                    .await
            }
        }
    }

    async fn complete_invalidated_activation(
        &self,
        actor: &SubjectId,
        mut intent: GrantActivationIntent,
        holder: String,
        now: &str,
    ) -> Result<(), SponsorshipError> {
        let expected_revision = intent.revision;
        intent.revision = next_generation(intent.revision, "grant_activation.revision")?;
        intent.state = GrantActivationState::Denied;
        intent.denial = Some(GrantActivationDenial::AuthorityInvalidated);
        intent.capacity_held = false;
        intent.lease = None;
        intent.updated_at = now.to_string();
        intent.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::GrantDenied,
            actor,
            SponsoredAuditTarget::Policy(intent.policy_id.clone()),
            Some(&intent.provider_request_id),
            "authority_invalidated",
            now,
        );
        self.store
            .complete_invalidated_grant_activation(CompleteInvalidatedGrantActivation {
                expected_revision,
                holder,
                intent,
                audit,
            })
            .await?;
        Ok(())
    }

    async fn cleanup_uncommitted_activation(
        &self,
        actor: &SubjectId,
        intent: GrantActivationIntent,
        holder: String,
        now: &str,
    ) -> Result<(), SponsorshipError> {
        let candidate = activation_cleanup_operation(&intent, now)?;
        let claim = self
            .store
            .claim_grant_activation_cleanup(ClaimGrantActivationCleanup {
                launch_id: intent.launch_id.clone(),
                holder: holder.clone(),
                operation: candidate,
            })
            .await?;
        if !claim.acquired {
            return if claim.intent.state == GrantActivationState::Cleaned {
                Ok(())
            } else {
                Err(SponsorshipError::ActivationInProgress)
            };
        }
        let mut intent = claim.intent;
        let receipt = intent.provider_receipt.clone().ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "activation cleanup has no encrypted provider receipt".into(),
            )
        })?;
        let policy = self.store.policy(&intent.policy_id).await?.ok_or_else(|| {
            SponsorshipStoreError::Malformed("activation cleanup has no sponsorship policy".into())
        })?;
        let stored_credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "activation cleanup has no provider credential".into(),
                )
            })?;
        stored_credential.validate()?;
        let credential_globally_revoked =
            stored_credential.record.material_state == SponsoredCredentialMaterialState::Retired;
        let credential = stored_credential.record;
        let secret = stored_credential.secret;
        let operation = intent.cleanup_operation.clone().ok_or_else(|| {
            SponsorshipStoreError::Malformed("activation cleanup identity is missing".into())
        })?;
        let resolution = if credential_globally_revoked {
            ProviderOperationResolution::Confirmed(())
        } else if operation.phase == ProviderOperationPhase::Prepared {
            let secret = secret.as_ref().ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "retained activation credential has no provider material".into(),
                )
            })?;
            intent = self
                .persist_activation_cleanup_phase(
                    intent,
                    &holder,
                    ProviderOperationPhase::Attempting,
                    Some(now.to_string()),
                    false,
                    now,
                )
                .await?;
            match self
                .provider
                .revoke_uncommitted_grant(
                    &operation.provider_request_id,
                    &receipt,
                    &credential,
                    secret,
                )
                .await
            {
                Ok(()) => ProviderOperationResolution::Confirmed(()),
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Unavailable) => {
                    self.persist_activation_cleanup_phase(
                        intent,
                        &holder,
                        ProviderOperationPhase::Prepared,
                        None,
                        true,
                        now,
                    )
                    .await?;
                    return Err(SponsorshipError::ProviderUnavailable);
                }
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else if matches!(
            operation.phase,
            ProviderOperationPhase::Attempting | ProviderOperationPhase::Ambiguous
        ) {
            let secret = secret.as_ref().ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "retained activation credential has no provider material".into(),
                )
            })?;
            match self
                .provider
                .reconcile_uncommitted_grant_revocation(
                    &operation.provider_request_id,
                    &receipt,
                    &credential,
                    secret,
                )
                .await
            {
                Ok(resolution) => resolution,
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::Unavailable)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else {
            return Err(SponsorshipError::ProviderRejected);
        };
        match resolution {
            ProviderOperationResolution::Confirmed(()) => {
                let expected_revision = intent.revision;
                intent.revision = next_generation(intent.revision, "grant_activation.revision")?;
                intent.state = GrantActivationState::Cleaned;
                intent.capacity_held = false;
                intent.lease = None;
                intent.cleanup_operation = None;
                intent.updated_at = now.to_string();
                intent.validate()?;
                let audit = SponsoredAuditEvent::privacy_safe(
                    &self.audit,
                    SponsoredAuditAction::UncommittedGrantRevoked,
                    actor,
                    SponsoredAuditTarget::Policy(intent.policy_id.clone()),
                    Some(&operation.provider_request_id),
                    "provider_authority_revoked",
                    now,
                );
                self.store
                    .complete_grant_activation_cleanup(CompleteGrantActivationCleanup {
                        expected_revision,
                        holder,
                        intent,
                        audit,
                    })
                    .await?;
                Ok(())
            }
            ProviderOperationResolution::Pending => {
                self.persist_activation_cleanup_phase(
                    intent,
                    &holder,
                    ProviderOperationPhase::Ambiguous,
                    Some(now.to_string()),
                    true,
                    now,
                )
                .await?;
                Err(SponsorshipError::ProviderAmbiguous)
            }
            ProviderOperationResolution::Rejected => {
                self.persist_activation_cleanup_phase(
                    intent,
                    &holder,
                    ProviderOperationPhase::Rejected,
                    Some(now.to_string()),
                    true,
                    now,
                )
                .await?;
                Err(SponsorshipError::ProviderRejected)
            }
        }
    }

    async fn persist_activation_cleanup_phase(
        &self,
        mut intent: GrantActivationIntent,
        holder: &str,
        phase: ProviderOperationPhase,
        attempted_at: Option<String>,
        release_lease: bool,
        now: &str,
    ) -> Result<GrantActivationIntent, SponsorshipError> {
        let expected = intent.revision;
        intent.revision = next_generation(intent.revision, "grant_activation.revision")?;
        let operation = intent.cleanup_operation.as_mut().ok_or_else(|| {
            SponsorshipStoreError::Malformed("activation cleanup identity is missing".into())
        })?;
        operation.phase = phase;
        operation.attempted_at = attempted_at;
        if release_lease {
            intent.lease = None;
        }
        intent.updated_at = now.to_string();
        intent.validate()?;
        Ok(self
            .store
            .update_grant_activation(expected, holder, &intent)
            .await?)
    }

    async fn apply_billing_resolution(
        &self,
        intent: GrantActivationIntent,
        holder: &str,
        resolution: ProviderOperationResolution<BillingAuthorization>,
        launch: &SponsoredLaunchAuthority,
        acceptance: &SponsoredGrantAcceptance,
        now: &str,
    ) -> Result<(GrantActivationIntent, bool), SponsorshipError> {
        match resolution {
            ProviderOperationResolution::Confirmed(BillingAuthorization::Confirmed) => {
                let intent = self
                    .begin_provider_grant_issue(intent, holder, launch, acceptance, now)
                    .await?;
                Ok((intent, true))
            }
            ProviderOperationResolution::Confirmed(BillingAuthorization::Rejected)
            | ProviderOperationResolution::Rejected => {
                self.advance_activation(
                    intent,
                    holder,
                    ActivationTransition {
                        state: GrantActivationState::Denied,
                        denial: Some(GrantActivationDenial::BillingRejected),
                        receipt: None,
                        capacity_held: false,
                        release_lease: true,
                    },
                    now,
                )
                .await?;
                Err(SponsorshipError::BillingRejected)
            }
            ProviderOperationResolution::Pending => {
                self.advance_activation(
                    intent,
                    holder,
                    ActivationTransition {
                        state: GrantActivationState::BillingAmbiguous,
                        denial: None,
                        receipt: None,
                        capacity_held: true,
                        release_lease: true,
                    },
                    now,
                )
                .await?;
                Err(SponsorshipError::BillingAmbiguous)
            }
        }
    }

    async fn begin_provider_grant_issue(
        &self,
        intent: GrantActivationIntent,
        holder: &str,
        launch: &SponsoredLaunchAuthority,
        acceptance: &SponsoredGrantAcceptance,
        now: &str,
    ) -> Result<GrantActivationIntent, SponsorshipError> {
        let transitioned = self
            .store
            .begin_provider_grant_issue(BeginProviderGrantIssue {
                launch_id: intent.launch_id.clone(),
                expected_revision: intent.revision,
                holder: holder.to_string(),
                launch_authority: launch.clone(),
                acceptance: acceptance.clone(),
                now: now.to_string(),
            })
            .await?;
        match transitioned.state {
            GrantActivationState::ProviderAttempting => Ok(transitioned),
            GrantActivationState::Denied
                if transitioned.denial == Some(GrantActivationDenial::AuthorityInvalidated) =>
            {
                Err(SponsorshipError::ScopeDenied)
            }
            _ => Err(SponsorshipStoreError::Malformed(
                "provider dispatch fence returned an invalid activation state".into(),
            )
            .into()),
        }
    }

    async fn advance_activation(
        &self,
        mut intent: GrantActivationIntent,
        holder: &str,
        transition: ActivationTransition,
        now: &str,
    ) -> Result<GrantActivationIntent, SponsorshipError> {
        let expected = intent.revision;
        intent.revision = next_generation(intent.revision, "grant_activation.revision")?;
        intent.state = transition.state;
        intent.denial = transition.denial;
        intent.capacity_held = transition.capacity_held;
        if let Some(receipt) = transition.receipt {
            intent.provider_receipt = Some(receipt);
        }
        if transition.release_lease {
            intent.lease = None;
        }
        intent.updated_at = now.to_string();
        intent.validate()?;
        Ok(self
            .store
            .update_grant_activation(expected, holder, &intent)
            .await?)
    }

    /// Fail closed for new spend before attempting provider revocation. A
    /// timeout leaves `RevocationPending`; the Launch/session/workspace remain
    /// intact so export and contributor-owned recovery stay available.
    pub async fn revoke_grant(
        &self,
        caller: &Caller,
        grant_id: &SponsoredGrantId,
        stable_request_id: &str,
    ) -> Result<SponsoredGrant, SponsorshipError> {
        self.mutate_grant_revocation(
            caller,
            grant_id,
            stable_request_id,
            ProviderOperationKind::RevokeGrant,
        )
        .await
    }

    pub async fn close_grant(
        &self,
        caller: &Caller,
        grant_id: &SponsoredGrantId,
        stable_request_id: &str,
    ) -> Result<SponsoredGrant, SponsorshipError> {
        require_contributor_capability(caller)?;
        let grant = self
            .store
            .grant(grant_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        let launch = self
            .store
            .launch_authority(&grant.launch_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        launch.validate()?;
        if launch.contributor != caller.subject
            || launch.state != SponsoredLaunchState::Closed
            || launch.launch_id != grant.launch_id
            || launch.session_id.as_ref() != Some(&grant.session_id)
        {
            return Err(SponsorshipError::Forbidden);
        }
        self.mutate_grant_revocation(
            caller,
            grant_id,
            stable_request_id,
            ProviderOperationKind::CloseGrant,
        )
        .await
    }

    async fn mutate_grant_revocation(
        &self,
        caller: &Caller,
        grant_id: &SponsoredGrantId,
        stable_request_id: &str,
        kind: ProviderOperationKind,
    ) -> Result<SponsoredGrant, SponsorshipError> {
        validate_stable_request_id(stable_request_id)?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let mut grant = self
            .store
            .grant(grant_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        let policy = self
            .store
            .policy(&grant.policy_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        require_grant_control(caller, &policy.owner, &grant.contributor)?;
        let final_state = match kind {
            ProviderOperationKind::RevokeGrant => SponsoredGrantState::Revoked,
            ProviderOperationKind::CloseGrant => SponsoredGrantState::Closed,
            ProviderOperationKind::RotateCredential
            | ProviderOperationKind::RevokeCredential
            | ProviderOperationKind::RevokeUncommittedGrant => {
                return Err(TriggerValidationError::field(
                    "provider_operation.kind",
                    "is not a grant revocation",
                )
                .into())
            }
        };
        if grant.state == final_state {
            return Ok(grant);
        }
        if matches!(
            grant.state,
            SponsoredGrantState::Expired
                | SponsoredGrantState::Exhausted
                | SponsoredGrantState::Revoked
                | SponsoredGrantState::Closed
        ) {
            return Err(SponsorshipError::GrantUnavailable);
        }
        if grant.state != SponsoredGrantState::RevocationPending {
            let expected = grant.generation;
            let payload_digest = canonical_digest(&(
                "cma-sponsored-grant-revocation-payload-v1",
                &grant.authority.provider_grant_id_digest,
                &grant.authority.revocation_handle_digest,
                grant.authority.provider_state_version,
            ))?;
            grant.generation = next_generation(grant.generation, "grant.generation")?;
            grant.state = SponsoredGrantState::RevocationPending;
            grant.pending_revocation = Some(PendingProviderOperation {
                kind,
                phase: ProviderOperationPhase::Prepared,
                provider_request_id: grant_operation_id(
                    kind,
                    &grant.id,
                    expected,
                    &payload_digest,
                )?,
                client_request_id_digest: sensitive_digest(
                    "cma-sponsored-grant-command-idempotency-v1",
                    stable_request_id,
                ),
                payload_digest,
                requested_at: now.clone(),
                attempted_at: None,
            });
            grant.updated_at = now.clone();
            grant.refresh_capability()?;
            grant.validate()?;
            let pending_audit = SponsoredAuditEvent::privacy_safe(
                &self.audit,
                SponsoredAuditAction::GrantRevocationRequested,
                &caller.subject,
                SponsoredAuditTarget::Grant {
                    policy_id: policy.id.clone(),
                    grant_id: grant.id.clone(),
                },
                Some(stable_request_id),
                "revocation_pending",
                now.clone(),
            );
            grant = self
                .store
                .replace_grant(expected, &grant, pending_audit)
                .await?;
        }
        let operation = grant.pending_revocation.clone().ok_or_else(|| {
            SponsorshipStoreError::Malformed("grant revocation identity is missing".into())
        })?;
        if operation.kind != kind {
            return Err(SponsorshipError::ProviderOperationInProgress);
        }
        let stored_credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or(SponsorshipError::CredentialUnavailable)?;
        stored_credential.validate()?;
        let credential_globally_revoked =
            stored_credential.record.material_state == SponsoredCredentialMaterialState::Retired;
        if operation.phase == ProviderOperationPhase::Rejected && !credential_globally_revoked {
            return Err(SponsorshipError::ProviderRejected);
        }
        let credential = stored_credential.record;
        let secret = stored_credential.secret;
        let receipt = if credential_globally_revoked {
            None
        } else {
            let receipt = self
                .store
                .provider_grant_receipt(&grant.id)
                .await?
                .ok_or_else(|| {
                    SponsorshipStoreError::Malformed(
                        "provider grant authority is missing from encrypted persistence".into(),
                    )
                })?;
            require_receipt_matches_grant(&receipt, &grant)?;
            Some(receipt)
        };
        let resolution = if credential_globally_revoked {
            ProviderOperationResolution::Confirmed(())
        } else if operation.phase == ProviderOperationPhase::Prepared {
            let secret = secret.as_ref().ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "retained grant credential has no provider material".into(),
                )
            })?;
            let receipt = receipt.as_ref().ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "retained grant credential has no provider receipt".into(),
                )
            })?;
            let expected = grant.generation;
            grant.generation = next_generation(grant.generation, "grant.generation")?;
            let pending = grant.pending_revocation.as_mut().ok_or_else(|| {
                SponsorshipStoreError::Malformed("grant revocation identity is missing".into())
            })?;
            pending.phase = ProviderOperationPhase::Attempting;
            pending.attempted_at = Some(now.clone());
            grant.updated_at = now.clone();
            grant.refresh_capability()?;
            grant.validate()?;
            let audit = SponsoredAuditEvent::privacy_safe(
                &self.audit,
                SponsoredAuditAction::GrantRevocationRequested,
                &caller.subject,
                SponsoredAuditTarget::Grant {
                    policy_id: policy.id.clone(),
                    grant_id: grant.id.clone(),
                },
                Some(&operation.provider_request_id),
                "revocation_attempting",
                now.clone(),
            );
            grant = self.store.replace_grant(expected, &grant, audit).await?;
            match self
                .provider
                .revoke_grant(
                    &operation.provider_request_id,
                    &grant,
                    receipt,
                    &credential,
                    secret,
                )
                .await
            {
                Ok(()) => ProviderOperationResolution::Confirmed(()),
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Unavailable) => {
                    self.persist_grant_operation_phase(
                        grant,
                        caller,
                        ProviderOperationPhase::Prepared,
                        None,
                        "revocation_deferred",
                        &now,
                    )
                    .await?;
                    return Err(SponsorshipError::ProviderUnavailable);
                }
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        } else {
            let secret = secret.as_ref().ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "retained grant credential has no provider material".into(),
                )
            })?;
            let receipt = receipt.as_ref().ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "retained grant credential has no provider receipt".into(),
                )
            })?;
            match self
                .provider
                .reconcile_grant_revocation(
                    &operation.provider_request_id,
                    &grant,
                    receipt,
                    &credential,
                    secret,
                )
                .await
            {
                Ok(resolution) => resolution,
                Err(SponsoredProviderError::Rejected) => ProviderOperationResolution::Rejected,
                Err(SponsoredProviderError::Ambiguous)
                | Err(SponsoredProviderError::Unavailable)
                | Err(SponsoredProviderError::MalformedEvidence) => {
                    ProviderOperationResolution::Pending
                }
            }
        };
        match resolution {
            ProviderOperationResolution::Confirmed(()) => {}
            ProviderOperationResolution::Pending => {
                self.persist_grant_operation_phase(
                    grant,
                    caller,
                    ProviderOperationPhase::Ambiguous,
                    Some(now.clone()),
                    "revocation_ambiguous",
                    &now,
                )
                .await?;
                return Err(SponsorshipError::ProviderAmbiguous);
            }
            ProviderOperationResolution::Rejected => {
                self.persist_grant_operation_phase(
                    grant,
                    caller,
                    ProviderOperationPhase::Rejected,
                    Some(now.clone()),
                    "revocation_rejected",
                    &now,
                )
                .await?;
                return Err(SponsorshipError::ProviderRejected);
            }
        }
        let expected = grant.generation;
        grant.state = final_state;
        grant.generation = next_generation(grant.generation, "grant.generation")?;
        grant.pending_revocation = None;
        grant.revoked_at = Some(now.clone());
        grant.updated_at = now.clone();
        grant.refresh_capability()?;
        grant.validate()?;
        let action = if final_state == SponsoredGrantState::Closed {
            SponsoredAuditAction::GrantClosed
        } else {
            SponsoredAuditAction::GrantRevoked
        };
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            action,
            &caller.subject,
            SponsoredAuditTarget::Grant {
                policy_id: policy.id,
                grant_id: grant.id.clone(),
            },
            Some(&operation.provider_request_id),
            if final_state == SponsoredGrantState::Closed {
                "closed"
            } else {
                "revoked"
            },
            now,
        );
        let reason = if final_state == SponsoredGrantState::Closed {
            GrantCapacityRelease::Closed
        } else {
            GrantCapacityRelease::Revoked
        };
        Ok(self
            .store
            .release_grant_capacity(expected, &grant, reason, audit)
            .await?)
    }

    pub async fn expire_grant(
        &self,
        caller: &Caller,
        grant_id: &SponsoredGrantId,
    ) -> Result<SponsoredGrant, SponsorshipError> {
        if !caller
            .authorization
            .allows(Capability::ReconcileSponsoredUsage)
            && !caller.authorization.allows(Capability::ConfigurePlatform)
        {
            return Err(SponsorshipError::Forbidden);
        }
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let mut grant = self
            .store
            .grant(grant_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        if matches!(
            grant.state,
            SponsoredGrantState::Expired
                | SponsoredGrantState::Exhausted
                | SponsoredGrantState::Revoked
                | SponsoredGrantState::Closed
        ) {
            return Ok(grant);
        }
        let expected = grant.generation;
        grant.generation = next_generation(grant.generation, "grant.generation")?;
        grant.state = SponsoredGrantState::Expired;
        grant.pending_revocation = None;
        grant.updated_at = now.clone();
        grant.refresh_capability()?;
        grant.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::GrantExpired,
            &caller.subject,
            SponsoredAuditTarget::Grant {
                policy_id: grant.policy_id.clone(),
                grant_id: grant.id.clone(),
            },
            None,
            "expired",
            now,
        );
        Ok(self
            .store
            .release_grant_capacity(expected, &grant, GrantCapacityRelease::Expired, audit)
            .await?)
    }

    async fn persist_credential_operation_phase(
        &self,
        mut credential: SponsoredCredentialRecord,
        caller: &Caller,
        phase: ProviderOperationPhase,
        attempted_at: Option<String>,
        outcome: &str,
        now: &str,
    ) -> Result<SponsoredCredentialRecord, SponsorshipError> {
        let expected = credential.generation;
        credential.generation = next_generation(credential.generation, "credential.generation")?;
        let request_id = {
            let operation = credential.pending_operation.as_mut().ok_or_else(|| {
                SponsorshipStoreError::Malformed("provider operation identity is missing".into())
            })?;
            operation.phase = phase;
            operation.attempted_at = attempted_at;
            operation.provider_request_id.clone()
        };
        credential.updated_at = now.to_string();
        credential.validate()?;
        let action = if credential.state == SponsoredCredentialState::RotationPending {
            SponsoredAuditAction::CredentialRotated
        } else {
            SponsoredAuditAction::CredentialRevocationRequested
        };
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            action,
            &caller.subject,
            SponsoredAuditTarget::Platform,
            Some(&request_id),
            outcome,
            now,
        );
        Ok(self
            .store
            .replace_credential(expected, &credential, None, audit)
            .await?)
    }

    async fn persist_grant_operation_phase(
        &self,
        mut grant: SponsoredGrant,
        caller: &Caller,
        phase: ProviderOperationPhase,
        attempted_at: Option<String>,
        outcome: &str,
        now: &str,
    ) -> Result<SponsoredGrant, SponsorshipError> {
        let expected = grant.generation;
        grant.generation = next_generation(grant.generation, "grant.generation")?;
        let request_id = {
            let operation = grant.pending_revocation.as_mut().ok_or_else(|| {
                SponsorshipStoreError::Malformed("grant revocation identity is missing".into())
            })?;
            operation.phase = phase;
            operation.attempted_at = attempted_at;
            operation.provider_request_id.clone()
        };
        grant.updated_at = now.to_string();
        grant.refresh_capability()?;
        grant.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::GrantRevocationRequested,
            &caller.subject,
            SponsoredAuditTarget::Grant {
                policy_id: grant.policy_id.clone(),
                grant_id: grant.id.clone(),
            },
            Some(&request_id),
            outcome,
            now,
        );
        Ok(self.store.replace_grant(expected, &grant, audit).await?)
    }
}
