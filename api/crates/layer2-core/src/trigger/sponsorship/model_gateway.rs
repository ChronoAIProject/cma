//! Provider-neutral sponsored model execution with durable accounting.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::lifecycle::{
    ReportSponsoredUsage, RequestSponsoredUsage, SponsorshipError, SponsorshipService,
};
#[cfg(test)]
use super::model::SponsoredUsageDispatchLease;
use super::model::{
    canonical_json_bytes, InertGrantCapability, ProviderCredentialSecret, ReservationDisposition,
    SponsoredCredentialState, SponsoredGrantState, SponsoredModelDispatchPayload,
    SponsoredModelRecoveryRecord, SponsoredModelResponsePayload, SponsoredQuota,
    SponsoredUsageReconciliationFence, SponsoredUsageReservation, SponsoredUsageReservationId,
    SponsoredUsageState, MAX_SPONSORED_MODEL_REQUEST_BYTES, MAX_SPONSORED_MODEL_RESPONSE_BYTES,
};
use super::ports::{ProviderGrantReceipt, SponsorshipStore, SponsorshipStoreError};
use super::validation::{
    model_usage_idempotency_fingerprint, require_receipt_matches_grant,
    require_stored_grant_policy_bindings, validate_digest, validate_policy_token,
    validate_provider_grant_receipt, validate_stable_request_id,
};
use crate::{
    Authorization, Caller, Capability, SponsorshipOperationalDomain,
    SponsorshipOperationalPseudonymSet, SubjectId, TriggerValidationError,
};

pub const SPONSORED_MODEL_GATEWAY_REQUEST_VERSION: u32 = 2;
/// Complete private-gateway envelope, including bounded authority metadata.
pub const MAX_SPONSORED_MODEL_GATEWAY_ENVELOPE_BYTES: usize =
    MAX_SPONSORED_MODEL_REQUEST_BYTES + 64 * 1024;
/// One bounded, non-streaming Responses API call selected by a runtime.
#[derive(Clone, PartialEq)]
pub struct SponsoredModelRequest {
    pub stable_request_id: String,
    pub request_digest: String,
    pub maximum_quota: SponsoredQuota,
    pub model: String,
    pub resource: String,
    pub request: Value,
}

impl std::fmt::Debug for SponsoredModelRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsoredModelRequest")
            .field("stable_request_id", &"<redacted>")
            .field("request_digest", &self.request_digest)
            .field("maximum_quota", &self.maximum_quota)
            .field("model", &self.model)
            .field("resource", &self.resource)
            .field("request", &"<redacted>")
            .finish()
    }
}

impl SponsoredModelRequest {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_stable_request_id(&self.stable_request_id)?;
        validate_digest("sponsored_model.request_digest", &self.request_digest)?;
        validate_policy_token("sponsored_model.model", &self.model)?;
        validate_policy_token("sponsored_model.resource", &self.resource)?;
        if self.resource != layer3_adapter::SPONSORED_MODEL_RESOURCE
            || self.maximum_quota.requests != 1
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.maximum_quota",
                "must reserve exactly one Responses request",
            ));
        }
        let object = self.request.as_object().ok_or_else(|| {
            TriggerValidationError::field("sponsored_model.request", "must be a JSON object")
        })?;
        if object.get("stream").is_some_and(|stream| stream != false) {
            return Err(TriggerValidationError::field(
                "sponsored_model.request.stream",
                "streaming is not supported",
            ));
        }
        if object.get("model").and_then(Value::as_str) != Some(self.model.as_str()) {
            return Err(TriggerValidationError::field(
                "sponsored_model.request.model",
                "must match the bounded model selection",
            ));
        }
        let maximum_output_tokens = object
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .filter(|tokens| *tokens > 0)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "sponsored_model.request.max_output_tokens",
                    "must be a positive integer",
                )
            })?;
        let canonical = canonical_json_bytes(&self.request)?;
        if canonical.len() > MAX_SPONSORED_MODEL_REQUEST_BYTES
            || self.maximum_quota.input_tokens != canonical.len().max(1) as u64
            || self.maximum_quota.output_tokens != maximum_output_tokens
            || lower_hex(Sha256::digest(&canonical)) != self.request_digest
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.request",
                "does not match its canonical digest or quota envelope",
            ));
        }
        let explicit = self
            .stable_request_id
            .strip_prefix("smr-e-")
            .filter(|identity| valid_lower_hex_digest(identity));
        let body_bound = self
            .stable_request_id
            .strip_prefix("smr-b-")
            .and_then(|value| value.split_once('-'))
            .filter(|(identity, digest)| {
                valid_lower_hex_digest(identity) && *digest == self.request_digest
            });
        if explicit.is_none() && body_bound.is_none() {
            return Err(TriggerValidationError::field(
                "sponsored_model.stable_request_id",
                "must be an opaque explicit identity or a canonical-body-bound identity",
            ));
        }
        Ok(())
    }
}

fn valid_lower_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Root provider authority and one independently scoped grant reference.
///
/// Neither field implements serialization. Only a provider adapter can expose
/// them, and the root credential must be used as bearer while the grant ID is
/// carried as structured request data.
pub struct SponsoredModelProviderAuthority {
    pub root_credential: ProviderCredentialSecret,
    pub grant_receipt: ProviderGrantReceipt,
}

impl std::fmt::Debug for SponsoredModelProviderAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsoredModelProviderAuthority")
            .field("root_credential", &"<redacted>")
            .field("grant_receipt", &"<redacted>")
            .finish()
    }
}

/// Immutable provider operation. Its ID is the durable reservation ID, never
/// a transient request-attempt identifier.
#[derive(Clone, PartialEq)]
pub struct SponsoredModelProviderRequest {
    pub operation_id: SponsoredUsageReservationId,
    pub request_digest: String,
    pub maximum_quota: SponsoredQuota,
    pub model: String,
    pub resource: String,
    pub request: Value,
}

impl SponsoredModelProviderRequest {
    fn validate(&self) -> Result<(), TriggerValidationError> {
        SponsoredUsageReservationId::parse(self.operation_id.0.clone())?;
        validate_digest("sponsored_model.request_digest", &self.request_digest)?;
        validate_policy_token("sponsored_model.model", &self.model)?;
        validate_policy_token("sponsored_model.resource", &self.resource)?;
        let object = self.request.as_object().ok_or_else(|| {
            TriggerValidationError::field("sponsored_model.request", "must be a JSON object")
        })?;
        let maximum_output_tokens = object
            .get("max_output_tokens")
            .and_then(Value::as_u64)
            .filter(|tokens| *tokens > 0)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "sponsored_model.request.max_output_tokens",
                    "must be a positive integer",
                )
            })?;
        let canonical = canonical_json_bytes(&self.request)?;
        if self.resource != layer3_adapter::SPONSORED_MODEL_RESOURCE
            || self.maximum_quota.requests != 1
            || self.maximum_quota.input_tokens != canonical.len().max(1) as u64
            || self.maximum_quota.output_tokens != maximum_output_tokens
            || object.get("model").and_then(Value::as_str) != Some(self.model.as_str())
            || object.get("stream").is_some_and(|stream| stream != false)
            || canonical.len() > MAX_SPONSORED_MODEL_REQUEST_BYTES
            || lower_hex(Sha256::digest(&canonical)) != self.request_digest
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.provider_request",
                "does not match its canonical digest, model, resource, or quota envelope",
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for SponsoredModelProviderRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsoredModelProviderRequest")
            .field("operation_id", &self.operation_id)
            .field("request_digest", &self.request_digest)
            .field("maximum_quota", &self.maximum_quota)
            .field("model", &self.model)
            .field("resource", &self.resource)
            .field("request", &"<redacted>")
            .finish()
    }
}

/// Confirmed bounded provider response and trusted token-metering observation.
#[derive(Clone, PartialEq)]
pub struct SponsoredModelProviderResponse {
    pub response: Value,
    pub actual_quota: SponsoredQuota,
}

impl std::fmt::Debug for SponsoredModelProviderResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsoredModelProviderResponse")
            .field("response", &"<redacted>")
            .field("actual_quota", &self.actual_quota)
            .finish()
    }
}

impl SponsoredModelProviderResponse {
    fn validate(&self, maximum: SponsoredQuota) -> Result<(), TriggerValidationError> {
        if !self.response.is_object()
            || serde_json::to_vec(&self.response)
                .map(|body| body.len() > MAX_SPONSORED_MODEL_RESPONSE_BYTES)
                .unwrap_or(true)
            || self.actual_quota.requests != 1
            || !self.actual_quota.fits_within(maximum)
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.provider_response",
                "must be a bounded response with usage inside the durable reservation",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum SponsoredModelProviderExecution {
    /// The immutable operation ID has one durable, linearizable terminal
    /// result. Exactly one effect has occurred. Every older in-flight and later
    /// execution must converge without another effect, and every later execute
    /// or observe call must return this exact response and actual quota.
    Confirmed(SponsoredModelProviderResponse),
    /// The immutable operation ID has a linearizable durable tombstone. When
    /// this is returned, no effect has occurred or can later occur for that
    /// ID, including from executions that began before the tombstone was
    /// observed; every later execution must also remain effect-free.
    Rejected,
    /// This call sent no bytes to the provider. It says nothing about an
    /// earlier execution of the same immutable operation ID.
    UnavailableBeforeDispatch,
    Ambiguous,
}

#[derive(Debug)]
pub enum SponsoredModelProviderObservation {
    /// The immutable operation ID has one durable, linearizable terminal
    /// result. Exactly one effect has occurred. Every older in-flight and later
    /// execution must converge without another effect, and every later execute
    /// or observe call must return this exact response and actual quota.
    Confirmed(SponsoredModelProviderResponse),
    /// The immutable operation ID has a linearizable durable tombstone. When
    /// this is returned, no effect has occurred or can later occur for that
    /// ID, including from executions that began before the tombstone was
    /// observed; every later execution must also remain effect-free.
    Rejected,
    Pending,
    /// No operation is currently observable. This is not a rejection
    /// tombstone and does not prove that another holder cannot still execute.
    NotFound,
}

#[async_trait]
pub trait SponsoredModelProvider: Send + Sync {
    /// Execute one immutable operation ID. `Confirmed` is a durable,
    /// linearizable one-effect result: older in-flight and later executions
    /// cannot produce another effect, and all later execute and observe calls
    /// return the exact same response and actual quota. Returning `Rejected` is
    /// a linearizable fence: no execution with that ID has produced an effect
    /// or can produce one later, including invocations already in flight when
    /// the tombstone is returned. All later calls remain effect-free as well.
    /// `UnavailableBeforeDispatch` only proves that this invocation sent no
    /// provider bytes.
    async fn execute(
        &self,
        provider: &str,
        authority: &SponsoredModelProviderAuthority,
        request: &SponsoredModelProviderRequest,
    ) -> SponsoredModelProviderExecution;

    /// Read back one deterministic operation without creating a replacement.
    /// `Confirmed` has the same durable, linearizable one-effect convergence
    /// contract as `execute`. `Rejected` has the same linearizable tombstone
    /// contract as `execute`, including fencing executions already in flight;
    /// `NotFound` explicitly does not.
    async fn observe(
        &self,
        provider: &str,
        authority: &SponsoredModelProviderAuthority,
        request: &SponsoredModelProviderRequest,
    ) -> SponsoredModelProviderObservation;
}

#[derive(Debug, thiserror::Error)]
pub enum SponsoredModelDataPlaneError {
    #[error("the sponsored model request is invalid: {0}")]
    InvalidRequest(#[from] TriggerValidationError),
    #[error("sponsored model authority or reservation was refused: {0}")]
    Sponsorship(#[from] SponsorshipError),
    #[error("the provider definitively rejected the model operation")]
    ProviderRejected,
    #[error("the provider was unavailable before dispatch")]
    ProviderUnavailable,
    #[error("the provider operation is pending or ambiguous")]
    ProviderAmbiguous,
    #[error("the stable sponsored model reservation was already released")]
    ReservationReleased,
    #[error("durable accounting could not record the provider outcome: {0}")]
    Accounting(SponsorshipError),
}

struct PreparedOperation {
    provider: String,
    authority: SponsoredModelProviderAuthority,
    capability: InertGrantCapability,
    request_digest: String,
    disposition: ReservationDisposition,
    reservation: SponsoredUsageReservation,
}

enum PreparedModelOperation {
    Operation(Box<PreparedOperation>),
    Settled(SponsoredModelProviderResponse),
    Released,
}

#[async_trait]
trait SponsoredModelAccounting: Send + Sync {
    async fn prepare(
        &self,
        capability: &InertGrantCapability,
        request: &SponsoredModelRequest,
    ) -> Result<PreparedModelOperation, SponsorshipError>;

    async fn claim(
        &self,
        capability: &InertGrantCapability,
        reservation: &SponsoredUsageReservation,
    ) -> Result<SponsoredUsageReservation, SponsorshipError>;

    async fn recover(
        &self,
        _id: &SponsoredUsageReservationId,
    ) -> Result<Option<(PreparedOperation, SponsoredModelProviderRequest)>, SponsorshipError> {
        Ok(None)
    }

    async fn reconcile(
        &self,
        reservation: &SponsoredUsageReservation,
        outcome: SponsoredUsageState,
        actual_quota: Option<SponsoredQuota>,
        dispatch_fence: Option<SponsoredUsageReconciliationFence>,
        model_response: Option<SponsoredModelResponsePayload>,
    ) -> Result<SponsoredUsageReservation, SponsorshipError>;
}

struct ProductionSponsoredModelAccounting {
    service: Arc<SponsorshipService>,
    store: Arc<dyn SponsorshipStore>,
}

impl ProductionSponsoredModelAccounting {
    fn existing_idempotency_fingerprint(
        contributor: &SubjectId,
        reservation: &SponsoredUsageReservation,
        capability: &InertGrantCapability,
        request: &SponsoredModelRequest,
    ) -> Result<String, SponsorshipError> {
        Ok(model_usage_idempotency_fingerprint(
            contributor,
            &RequestSponsoredUsage {
                stable_request_id: request.stable_request_id.clone(),
                grant_id: reservation.grant_id.clone(),
                provider: reservation.price_quote.provider.clone(),
                model: reservation.price_quote.model.clone(),
                resource: reservation.price_quote.resource.clone(),
                maximum_quota: reservation.reserved.quota,
                capability: capability.clone(),
            },
            &request.request_digest,
        )?)
    }

    async fn prepare_existing(
        &self,
        reservation: SponsoredUsageReservation,
        capability: &InertGrantCapability,
        idempotency_fingerprint: &str,
        request: &SponsoredModelRequest,
    ) -> Result<PreparedModelOperation, SponsorshipError> {
        reservation.validate()?;
        if reservation.idempotency_fingerprint != idempotency_fingerprint
            || reservation.grant_id != capability.grant_id
        {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        if reservation.price_quote.model != request.model
            || reservation.price_quote.resource != request.resource
            || reservation.reserved.quota != request.maximum_quota
        {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        match reservation.state {
            SponsoredUsageState::Settled => {
                let payload = self
                    .store
                    .model_usage_settled_response(&reservation.id)
                    .await?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "settled sponsored model reservation has no response custody".into(),
                        )
                    })?;
                payload.validate()?;
                if payload.request_digest() != request.request_digest
                    || reservation.actual.as_ref().map(|actual| actual.quota)
                        != Some(payload.actual_quota())
                {
                    return Err(SponsorshipStoreError::Malformed(
                        "settled sponsored model response does not match its reservation".into(),
                    )
                    .into());
                }
                let response = SponsoredModelProviderResponse {
                    response: payload.response()?,
                    actual_quota: payload.actual_quota(),
                };
                response.validate(reservation.reserved.quota)?;
                Ok(PreparedModelOperation::Settled(response))
            }
            SponsoredUsageState::Released => Ok(PreparedModelOperation::Released),
            SponsoredUsageState::Reserved
            | SponsoredUsageState::Attempting
            | SponsoredUsageState::Ambiguous => {
                let record = self
                    .store
                    .model_usage_recovery_record(&reservation.id)
                    .await?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "nonterminal sponsored model reservation has no request custody".into(),
                        )
                    })?;
                if record.reservation != reservation
                    || record.dispatch.request_digest() != request.request_digest
                {
                    return Err(SponsorshipStoreError::Conflict.into());
                }
                let (prepared, recovered_request) = self.prepare_recovery(record).await?;
                if recovered_request.request_digest != request.request_digest
                    || recovered_request.request != request.request
                {
                    return Err(SponsorshipStoreError::Conflict.into());
                }
                Ok(PreparedModelOperation::Operation(Box::new(prepared)))
            }
        }
    }

    /// Alias resolution and encrypted-custody reads are separate store calls.
    /// Reclassify once when a claim or terminal transition wins between them;
    /// a second concurrent change is surfaced as contention instead of false
    /// corruption. Stable payload and authority errors are preserved.
    async fn prepare_existing_converging(
        &self,
        first: SponsoredUsageReservation,
        request_identity: &SponsorshipOperationalPseudonymSet,
        contributor: &SubjectId,
        capability: &InertGrantCapability,
        request: &SponsoredModelRequest,
    ) -> Result<PreparedModelOperation, SponsorshipError> {
        let first_fingerprint =
            Self::existing_idempotency_fingerprint(contributor, &first, capability, request)?;
        let first_error = match self
            .prepare_existing(first.clone(), capability, &first_fingerprint, request)
            .await
        {
            Ok(prepared) => return Ok(prepared),
            Err(error) => error,
        };

        let Some(second) = self
            .store
            .usage_reservation_for_request(request_identity)
            .await?
        else {
            return Err(first_error);
        };
        if second == first {
            return Err(first_error);
        }

        let second_fingerprint =
            Self::existing_idempotency_fingerprint(contributor, &second, capability, request)?;
        let second_error = match self
            .prepare_existing(second.clone(), capability, &second_fingerprint, request)
            .await
        {
            Ok(prepared) => return Ok(prepared),
            Err(error) => error,
        };
        match self
            .store
            .usage_reservation_for_request(request_identity)
            .await?
        {
            Some(latest) if latest != second => Err(SponsorshipError::ProviderOperationInProgress),
            Some(_) | None => Err(second_error),
        }
    }

    async fn prepare_recovery(
        &self,
        record: SponsoredModelRecoveryRecord,
    ) -> Result<(PreparedOperation, SponsoredModelProviderRequest), SponsorshipError> {
        record.reservation.validate()?;
        record.dispatch.validate()?;
        if !matches!(
            record.reservation.state,
            SponsoredUsageState::Reserved
                | SponsoredUsageState::Attempting
                | SponsoredUsageState::Ambiguous
        ) {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        let grant = self
            .store
            .grant(&record.reservation.grant_id)
            .await?
            .ok_or(SponsorshipError::GrantUnavailable)?;
        grant.validate()?;
        let policy = self.store.policy(&grant.policy_id).await?.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "sponsored model recovery grant references a missing policy".into(),
            )
        })?;
        require_stored_grant_policy_bindings(&grant, &policy)?;
        let stored_credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or(SponsorshipError::CredentialUnavailable)?;
        let (credential, root_credential) = stored_credential.into_required_secret()?;
        if !credential.permits_retained_model_recovery_authority()
            || credential.id != policy.credential_id
            || credential.authority.provider != policy.provider_policy.provider
            || credential.authority.provider_policy_key
                != policy.provider_policy.provider_policy_key
            || record.reservation.price_quote.provider != policy.provider_policy.provider
        {
            return Err(SponsorshipError::CredentialUnavailable);
        }
        let grant_receipt = self
            .store
            .provider_grant_receipt(&grant.id)
            .await?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "sponsored model recovery grant has no encrypted provider receipt".into(),
                )
            })?;
        require_receipt_matches_grant(&self.service.operational, &grant_receipt, &grant)?;

        let provider_request = SponsoredModelProviderRequest {
            operation_id: record.reservation.id.clone(),
            request_digest: record.dispatch.request_digest().to_string(),
            maximum_quota: record.reservation.reserved.quota,
            model: record.reservation.price_quote.model.clone(),
            resource: record.reservation.price_quote.resource.clone(),
            request: record.dispatch.request()?,
        };
        provider_request.validate()?;
        Ok((
            PreparedOperation {
                provider: record.reservation.price_quote.provider.clone(),
                authority: SponsoredModelProviderAuthority {
                    root_credential,
                    grant_receipt,
                },
                capability: grant.capability,
                request_digest: provider_request.request_digest.clone(),
                disposition: ReservationDisposition::Existing,
                reservation: record.reservation,
            },
            provider_request,
        ))
    }
}

#[async_trait]
impl SponsoredModelAccounting for ProductionSponsoredModelAccounting {
    async fn prepare(
        &self,
        capability: &InertGrantCapability,
        request: &SponsoredModelRequest,
    ) -> Result<PreparedModelOperation, SponsorshipError> {
        capability.validate()?;
        let grant = self
            .store
            .grant(&capability.grant_id)
            .await?
            .ok_or(SponsorshipError::GrantUnavailable)?;
        grant.validate()?;
        let request_identity = self.service.operational.pseudonymize(
            SponsorshipOperationalDomain::UsageRequest,
            &[
                &grant.id.0,
                &grant.contributor.0,
                &request.stable_request_id,
            ],
        )?;
        if let Some(existing) = self
            .store
            .usage_reservation_for_request(&request_identity)
            .await?
        {
            return self
                .prepare_existing_converging(
                    existing,
                    &request_identity,
                    &grant.contributor,
                    capability,
                    request,
                )
                .await;
        }

        let policy = self.store.policy(&grant.policy_id).await?.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "sponsored model grant references a missing policy".into(),
            )
        })?;
        require_stored_grant_policy_bindings(&grant, &policy)?;
        let usage_command = RequestSponsoredUsage {
            stable_request_id: request.stable_request_id.clone(),
            grant_id: grant.id.clone(),
            provider: policy.provider_policy.provider.clone(),
            model: request.model.clone(),
            resource: request.resource.clone(),
            maximum_quota: request.maximum_quota,
            capability: capability.clone(),
        };
        if grant.state != SponsoredGrantState::Active || grant.capability != *capability {
            return Err(SponsorshipError::GrantUnavailable);
        }
        if grant.policy_generation != policy.generation
            || !policy
                .provider_policy
                .allowed_models
                .contains(&request.model)
            || !policy
                .provider_policy
                .allowed_resources
                .contains(&request.resource)
        {
            return Err(SponsorshipError::GrantUnavailable);
        }
        let stored_credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or(SponsorshipError::CredentialUnavailable)?;
        let (credential, root_credential) = stored_credential.into_required_secret()?;
        if credential.state != SponsoredCredentialState::Active
            || credential.id != policy.credential_id
            || credential.authority.provider != policy.provider_policy.provider
            || credential.authority.provider_policy_key
                != policy.provider_policy.provider_policy_key
        {
            return Err(SponsorshipError::CredentialUnavailable);
        }
        let grant_receipt = self
            .store
            .provider_grant_receipt(&grant.id)
            .await?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "sponsored model grant has no encrypted provider receipt".into(),
                )
            })?;
        let now = self.service.clock.now();
        validate_provider_grant_receipt(&grant_receipt, &policy, &now)?;
        require_receipt_matches_grant(&self.service.operational, &grant_receipt, &grant)?;

        let caller = Caller::new(
            grant.contributor.clone(),
            Authorization::from_capabilities([Capability::UseOwnSessions]),
        );
        let outcome = self
            .service
            .reserve_model_usage(
                &caller,
                usage_command,
                SponsoredModelDispatchPayload::from_request(
                    request.request_digest.clone(),
                    &request.request,
                )?,
            )
            .await?;
        outcome.reservation.validate()?;
        if outcome.disposition == ReservationDisposition::Existing {
            return self
                .prepare_existing_converging(
                    outcome.reservation,
                    &request_identity,
                    &grant.contributor,
                    capability,
                    request,
                )
                .await;
        }
        if outcome.reservation.grant_id != grant.id
            || outcome.reservation.price_quote.provider != policy.provider_policy.provider
            || outcome.reservation.price_quote.model != request.model
            || outcome.reservation.price_quote.resource != request.resource
            || outcome.reservation.reserved.quota != request.maximum_quota
        {
            return Err(SponsorshipStoreError::Malformed(
                "sponsored model reservation does not match its provider request".into(),
            )
            .into());
        }
        Ok(PreparedModelOperation::Operation(Box::new(
            PreparedOperation {
                provider: policy.provider_policy.provider,
                authority: SponsoredModelProviderAuthority {
                    root_credential,
                    grant_receipt,
                },
                capability: grant.capability,
                request_digest: request.request_digest.clone(),
                disposition: outcome.disposition,
                reservation: outcome.reservation,
            },
        )))
    }

    async fn claim(
        &self,
        capability: &InertGrantCapability,
        reservation: &SponsoredUsageReservation,
    ) -> Result<SponsoredUsageReservation, SponsorshipError> {
        self.service
            .claim_model_usage_dispatch(capability, reservation)
            .await
    }

    async fn recover(
        &self,
        id: &SponsoredUsageReservationId,
    ) -> Result<Option<(PreparedOperation, SponsoredModelProviderRequest)>, SponsorshipError> {
        let Some(record) = self.store.model_usage_recovery_record(id).await? else {
            return Ok(None);
        };
        self.prepare_recovery(record).await.map(Some)
    }

    async fn reconcile(
        &self,
        reservation: &SponsoredUsageReservation,
        outcome: SponsoredUsageState,
        actual_quota: Option<SponsoredQuota>,
        dispatch_fence: Option<SponsoredUsageReconciliationFence>,
        model_response: Option<SponsoredModelResponsePayload>,
    ) -> Result<SponsoredUsageReservation, SponsorshipError> {
        let caller = Caller::new(
            SubjectId("cma-sponsored-model-meter".into()),
            Authorization::from_capabilities([Capability::ReconcileSponsoredUsage]),
        );
        self.service
            .reconcile_model_usage(
                &caller,
                ReportSponsoredUsage {
                    reservation_id: reservation.id.clone(),
                    request_fingerprint: reservation.request_fingerprint.clone(),
                    actual_quota,
                    outcome,
                },
                dispatch_fence,
                model_response,
            )
            .await
    }
}

pub struct SponsoredModelDataPlaneService {
    accounting: Arc<dyn SponsoredModelAccounting>,
    provider: Arc<dyn SponsoredModelProvider>,
}

impl SponsoredModelDataPlaneService {
    pub fn new(
        service: Arc<SponsorshipService>,
        store: Arc<dyn SponsorshipStore>,
        provider: Arc<dyn SponsoredModelProvider>,
    ) -> Self {
        Self {
            accounting: Arc::new(ProductionSponsoredModelAccounting { service, store }),
            provider,
        }
    }

    pub async fn execute(
        &self,
        capability: &InertGrantCapability,
        request: SponsoredModelRequest,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
        request.validate()?;
        capability.validate()?;
        let prepared = match self.accounting.prepare(capability, &request).await? {
            PreparedModelOperation::Operation(prepared) => *prepared,
            PreparedModelOperation::Settled(response) => return Ok(response),
            PreparedModelOperation::Released => {
                return Err(SponsoredModelDataPlaneError::ReservationReleased)
            }
        };
        let provider_request = SponsoredModelProviderRequest {
            operation_id: prepared.reservation.id.clone(),
            request_digest: request.request_digest,
            maximum_quota: request.maximum_quota,
            model: request.model,
            resource: request.resource,
            request: request.request,
        };

        if prepared.disposition == ReservationDisposition::Existing {
            return self.observe_existing(prepared, &provider_request).await;
        }
        if prepared.reservation.state != SponsoredUsageState::Reserved {
            return Err(SponsoredModelDataPlaneError::Accounting(
                SponsorshipStoreError::Malformed(
                    "a new sponsored model reservation was not reserved".into(),
                )
                .into(),
            ));
        }
        let prepared = self.claim_first_dispatch(prepared).await?;
        self.dispatch(prepared, &provider_request).await
    }

    /// Recover one durable model dispatch without requiring live-spend
    /// authority for observation. A provider call is possible only after a
    /// fresh atomic claim; `Pending` observations are strictly read-only.
    pub async fn recover_usage(
        &self,
        id: &SponsoredUsageReservationId,
    ) -> Result<Option<SponsoredUsageState>, SponsoredModelDataPlaneError> {
        SponsoredUsageReservationId::parse(id.0.clone())?;
        let Some((prepared, request)) = self.accounting.recover(id).await? else {
            return Ok(None);
        };
        request.validate()?;
        match prepared.reservation.state {
            SponsoredUsageState::Reserved => match self.claim(prepared).await {
                Ok(prepared) => self.dispatch_recovery(prepared, &request).await.map(Some),
                Err(SponsoredModelDataPlaneError::Sponsorship(error))
                    if redispatch_definitively_refused(&error) =>
                {
                    let Some((current, _)) = self.accounting.recover(id).await? else {
                        return Ok(Some(SponsoredUsageState::Released));
                    };
                    let released = self
                        .accounting
                        .reconcile(
                            &current.reservation,
                            SponsoredUsageState::Released,
                            None,
                            Some(SponsoredUsageReconciliationFence::NeverDispatched),
                            None,
                        )
                        .await
                        .map_err(SponsoredModelDataPlaneError::Accounting)?;
                    Ok(Some(released.state))
                }
                Err(error) => Err(error),
            },
            SponsoredUsageState::Attempting | SponsoredUsageState::Ambiguous => {
                match self
                    .provider
                    .observe(&prepared.provider, &prepared.authority, &request)
                    .await
                {
                    SponsoredModelProviderObservation::Confirmed(response) => {
                        if response_reflects_provider_authority(
                            &response.response,
                            &prepared.authority,
                        ) || response
                            .validate(prepared.reservation.reserved.quota)
                            .is_err()
                        {
                            return Err(SponsoredModelDataPlaneError::ProviderAmbiguous);
                        }
                        let settled = self
                            .accounting
                            .reconcile(
                                &prepared.reservation,
                                SponsoredUsageState::Settled,
                                Some(response.actual_quota),
                                provider_observation_fence(&prepared.reservation),
                                Some(SponsoredModelResponsePayload::from_response(
                                    request.request_digest.clone(),
                                    response.actual_quota,
                                    &response.response,
                                )?),
                            )
                            .await
                            .map_err(SponsoredModelDataPlaneError::Accounting)?;
                        Ok(Some(settled.state))
                    }
                    SponsoredModelProviderObservation::Rejected => {
                        let released = self
                            .accounting
                            .reconcile(
                                &prepared.reservation,
                                SponsoredUsageState::Released,
                                None,
                                provider_observation_fence(&prepared.reservation),
                                None,
                            )
                            .await
                            .map_err(SponsoredModelDataPlaneError::Accounting)?;
                        Ok(Some(released.state))
                    }
                    SponsoredModelProviderObservation::Pending => {
                        Ok(Some(prepared.reservation.state))
                    }
                    SponsoredModelProviderObservation::NotFound => {
                        self.recover_not_found(prepared, &request).await.map(Some)
                    }
                }
            }
            SponsoredUsageState::Settled | SponsoredUsageState::Released => Ok(None),
        }
    }

    async fn recover_not_found(
        &self,
        prepared: PreparedOperation,
        request: &SponsoredModelProviderRequest,
    ) -> Result<SponsoredUsageState, SponsoredModelDataPlaneError> {
        let observed_state = prepared.reservation.state;
        match self.claim(prepared).await {
            Ok(claimed) => self.dispatch_recovery(claimed, request).await,
            Err(SponsoredModelDataPlaneError::Sponsorship(
                SponsorshipError::ProviderOperationInProgress,
            )) => Ok(observed_state),
            Err(SponsoredModelDataPlaneError::Sponsorship(error))
                if redispatch_definitively_refused(&error) =>
            {
                Ok(observed_state)
            }
            Err(error) => Err(error),
        }
    }

    async fn dispatch_recovery(
        &self,
        prepared: PreparedOperation,
        request: &SponsoredModelProviderRequest,
    ) -> Result<SponsoredUsageState, SponsoredModelDataPlaneError> {
        let dispatch_fence = dispatch_claim_fence(&prepared.reservation)?;
        let (outcome, actual_quota, model_response) = match self
            .provider
            .execute(&prepared.provider, &prepared.authority, request)
            .await
        {
            SponsoredModelProviderExecution::Confirmed(response)
                if !response_reflects_provider_authority(
                    &response.response,
                    &prepared.authority,
                ) && response
                    .validate(prepared.reservation.reserved.quota)
                    .is_ok() =>
            {
                let payload = SponsoredModelResponsePayload::from_response(
                    request.request_digest.clone(),
                    response.actual_quota,
                    &response.response,
                )?;
                (
                    SponsoredUsageState::Settled,
                    Some(response.actual_quota),
                    Some(payload),
                )
            }
            SponsoredModelProviderExecution::Rejected => {
                (SponsoredUsageState::Released, None, None)
            }
            SponsoredModelProviderExecution::UnavailableBeforeDispatch
                if prepared.reservation.dispatch_revision == 1 =>
            {
                (SponsoredUsageState::Released, None, None)
            }
            SponsoredModelProviderExecution::UnavailableBeforeDispatch => {
                (SponsoredUsageState::Ambiguous, None, None)
            }
            SponsoredModelProviderExecution::Confirmed(_)
            | SponsoredModelProviderExecution::Ambiguous => {
                (SponsoredUsageState::Ambiguous, None, None)
            }
        };
        let reconciled = self
            .accounting
            .reconcile(
                &prepared.reservation,
                outcome,
                actual_quota,
                Some(dispatch_fence),
                model_response,
            )
            .await
            .map_err(SponsoredModelDataPlaneError::Accounting)?;
        Ok(reconciled.state)
    }

    async fn claim(
        &self,
        mut prepared: PreparedOperation,
    ) -> Result<PreparedOperation, SponsoredModelDataPlaneError> {
        let claimed = self
            .accounting
            .claim(&prepared.capability, &prepared.reservation)
            .await?;
        if claimed.state != SponsoredUsageState::Attempting
            || claimed.id != prepared.reservation.id
            || claimed.request_fingerprint != prepared.reservation.request_fingerprint
        {
            return Err(SponsoredModelDataPlaneError::Accounting(
                SponsorshipStoreError::Malformed(
                    "sponsored model dispatch claim returned an inconsistent reservation".into(),
                )
                .into(),
            ));
        }
        prepared.reservation = claimed;
        Ok(prepared)
    }

    async fn claim_first_dispatch(
        &self,
        prepared: PreparedOperation,
    ) -> Result<PreparedOperation, SponsoredModelDataPlaneError> {
        let reservation = prepared.reservation.clone();
        match self.claim(prepared).await {
            Ok(claimed) => Ok(claimed),
            Err(SponsoredModelDataPlaneError::Sponsorship(error))
                if redispatch_definitively_refused(&error) =>
            {
                self.accounting
                    .reconcile(
                        &reservation,
                        SponsoredUsageState::Released,
                        None,
                        Some(SponsoredUsageReconciliationFence::NeverDispatched),
                        None,
                    )
                    .await
                    .map_err(SponsoredModelDataPlaneError::Accounting)?;
                Err(SponsoredModelDataPlaneError::Sponsorship(error))
            }
            Err(error) => Err(error),
        }
    }

    async fn observe_existing(
        &self,
        prepared: PreparedOperation,
        request: &SponsoredModelProviderRequest,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
        if prepared.reservation.state == SponsoredUsageState::Released {
            return Err(SponsoredModelDataPlaneError::ReservationReleased);
        }
        match self
            .provider
            .observe(&prepared.provider, &prepared.authority, request)
            .await
        {
            SponsoredModelProviderObservation::Confirmed(response) => {
                let fence = provider_observation_fence(&prepared.reservation);
                self.settle_confirmed(prepared, response, fence).await
            }
            SponsoredModelProviderObservation::Rejected
                if !matches!(prepared.reservation.state, SponsoredUsageState::Settled) =>
            {
                let fence = provider_observation_fence(&prepared.reservation);
                self.finish_without_response(
                    prepared,
                    SponsoredUsageState::Released,
                    fence,
                    SponsoredModelDataPlaneError::ProviderRejected,
                )
                .await
            }
            SponsoredModelProviderObservation::Pending
                if !matches!(prepared.reservation.state, SponsoredUsageState::Settled) =>
            {
                Err(SponsoredModelDataPlaneError::ProviderAmbiguous)
            }
            SponsoredModelProviderObservation::NotFound
                if matches!(
                    prepared.reservation.state,
                    SponsoredUsageState::Reserved
                        | SponsoredUsageState::Attempting
                        | SponsoredUsageState::Ambiguous
                ) =>
            {
                self.dispatch_after_not_found(prepared, request).await
            }
            SponsoredModelProviderObservation::Rejected
            | SponsoredModelProviderObservation::Pending
            | SponsoredModelProviderObservation::NotFound => {
                Err(SponsoredModelDataPlaneError::ProviderAmbiguous)
            }
        }
    }

    async fn dispatch_after_not_found(
        &self,
        prepared: PreparedOperation,
        request: &SponsoredModelProviderRequest,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
        let observed = prepared.reservation.clone();
        match self.claim(prepared).await {
            Ok(claimed) => self.dispatch(claimed, request).await,
            Err(SponsoredModelDataPlaneError::Sponsorship(
                SponsorshipError::ProviderOperationInProgress,
            )) => Err(SponsoredModelDataPlaneError::ProviderAmbiguous),
            Err(SponsoredModelDataPlaneError::Sponsorship(error))
                if redispatch_definitively_refused(&error) =>
            {
                if observed.state == SponsoredUsageState::Reserved
                    && observed.dispatch_revision == 0
                    && observed.dispatch_lease.is_none()
                {
                    self.accounting
                        .reconcile(
                            &observed,
                            SponsoredUsageState::Released,
                            None,
                            Some(SponsoredUsageReconciliationFence::NeverDispatched),
                            None,
                        )
                        .await
                        .map_err(SponsoredModelDataPlaneError::Accounting)?;
                    return Err(SponsoredModelDataPlaneError::ReservationReleased);
                }
                Err(SponsoredModelDataPlaneError::ProviderAmbiguous)
            }
            Err(error) => Err(error),
        }
    }

    async fn dispatch(
        &self,
        prepared: PreparedOperation,
        request: &SponsoredModelProviderRequest,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
        request.validate()?;
        let dispatch_fence = dispatch_claim_fence(&prepared.reservation)?;
        match self
            .provider
            .execute(&prepared.provider, &prepared.authority, request)
            .await
        {
            SponsoredModelProviderExecution::Confirmed(response) => {
                self.settle_confirmed(prepared, response, Some(dispatch_fence))
                    .await
            }
            SponsoredModelProviderExecution::Rejected => {
                self.finish_without_response(
                    prepared,
                    SponsoredUsageState::Released,
                    Some(dispatch_fence),
                    SponsoredModelDataPlaneError::ProviderRejected,
                )
                .await
            }
            SponsoredModelProviderExecution::UnavailableBeforeDispatch => {
                if prepared.reservation.dispatch_revision == 1 {
                    self.finish_without_response(
                        prepared,
                        SponsoredUsageState::Released,
                        Some(dispatch_fence),
                        SponsoredModelDataPlaneError::ProviderUnavailable,
                    )
                    .await
                } else {
                    self.finish_without_response(
                        prepared,
                        SponsoredUsageState::Ambiguous,
                        Some(dispatch_fence),
                        SponsoredModelDataPlaneError::ProviderAmbiguous,
                    )
                    .await
                }
            }
            SponsoredModelProviderExecution::Ambiguous => {
                self.finish_without_response(
                    prepared,
                    SponsoredUsageState::Ambiguous,
                    Some(dispatch_fence),
                    SponsoredModelDataPlaneError::ProviderAmbiguous,
                )
                .await
            }
        }
    }

    async fn settle_confirmed(
        &self,
        prepared: PreparedOperation,
        response: SponsoredModelProviderResponse,
        dispatch_fence: Option<SponsoredUsageReconciliationFence>,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
        if response_reflects_provider_authority(&response.response, &prepared.authority)
            || response
                .validate(prepared.reservation.reserved.quota)
                .is_err()
        {
            if matches!(
                dispatch_fence,
                Some(SponsoredUsageReconciliationFence::ProviderObservation { .. })
            ) {
                return Err(SponsoredModelDataPlaneError::ProviderAmbiguous);
            }
            return self
                .finish_without_response(
                    prepared,
                    SponsoredUsageState::Ambiguous,
                    dispatch_fence,
                    SponsoredModelDataPlaneError::ProviderAmbiguous,
                )
                .await;
        }
        let model_response = SponsoredModelResponsePayload::from_response(
            prepared.request_digest.clone(),
            response.actual_quota,
            &response.response,
        )?;
        let settled = self
            .accounting
            .reconcile(
                &prepared.reservation,
                SponsoredUsageState::Settled,
                Some(response.actual_quota),
                dispatch_fence,
                Some(model_response),
            )
            .await
            .map_err(SponsoredModelDataPlaneError::Accounting)?;
        if settled.state != SponsoredUsageState::Settled
            || settled.actual.as_ref().map(|actual| actual.quota) != Some(response.actual_quota)
        {
            return Err(SponsoredModelDataPlaneError::Accounting(
                SponsorshipStoreError::Malformed(
                    "sponsored model settlement did not durably record provider usage".into(),
                )
                .into(),
            ));
        }
        Ok(response)
    }

    async fn finish_without_response(
        &self,
        prepared: PreparedOperation,
        outcome: SponsoredUsageState,
        dispatch_fence: Option<SponsoredUsageReconciliationFence>,
        result: SponsoredModelDataPlaneError,
    ) -> Result<SponsoredModelProviderResponse, SponsoredModelDataPlaneError> {
        let reconciled = self
            .accounting
            .reconcile(&prepared.reservation, outcome, None, dispatch_fence, None)
            .await
            .map_err(SponsoredModelDataPlaneError::Accounting)?;
        if reconciled.state != outcome || reconciled.actual.is_some() {
            return Err(SponsoredModelDataPlaneError::Accounting(
                SponsorshipStoreError::Malformed(
                    "sponsored model outcome was not durably reconciled".into(),
                )
                .into(),
            ));
        }
        Err(result)
    }
}

fn response_reflects_provider_authority(
    response: &Value,
    authority: &SponsoredModelProviderAuthority,
) -> bool {
    let protected = [
        authority.root_credential.expose(),
        authority
            .grant_receipt
            .provider_grant_id
            .expose_for_provider_or_encryption(),
        authority
            .grant_receipt
            .revocation_handle
            .expose_for_provider_or_encryption(),
    ];
    protected
        .iter()
        .any(|protected| json_contains_text(response, protected))
}

fn json_contains_text(value: &Value, protected: &str) -> bool {
    if protected.is_empty() {
        return false;
    }
    match value {
        Value::String(value) => value.contains(protected),
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_text(value, protected)),
        Value::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains(protected) || json_contains_text(value, protected)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn dispatch_claim_fence(
    reservation: &SponsoredUsageReservation,
) -> Result<SponsoredUsageReconciliationFence, SponsoredModelDataPlaneError> {
    let lease = reservation.dispatch_lease.as_ref().ok_or_else(|| {
        SponsoredModelDataPlaneError::Accounting(
            SponsorshipStoreError::Malformed(
                "attempting sponsored model reservation has no dispatch lease".into(),
            )
            .into(),
        )
    })?;
    if reservation.state != SponsoredUsageState::Attempting || reservation.dispatch_revision == 0 {
        return Err(SponsoredModelDataPlaneError::Accounting(
            SponsorshipStoreError::Malformed(
                "provider execution did not hold a durable dispatch claim".into(),
            )
            .into(),
        ));
    }
    Ok(SponsoredUsageReconciliationFence::DispatchClaim {
        revision: reservation.dispatch_revision,
        holder: lease.holder.clone(),
    })
}

fn provider_observation_fence(
    reservation: &SponsoredUsageReservation,
) -> Option<SponsoredUsageReconciliationFence> {
    matches!(
        reservation.state,
        SponsoredUsageState::Reserved
            | SponsoredUsageState::Attempting
            | SponsoredUsageState::Ambiguous
    )
    .then_some(SponsoredUsageReconciliationFence::ProviderObservation {
        state: reservation.state,
        revision: reservation.dispatch_revision,
    })
}

fn redispatch_definitively_refused(error: &SponsorshipError) -> bool {
    matches!(
        error,
        SponsorshipError::Disabled
            | SponsorshipError::KillSwitch
            | SponsorshipError::PolicyUnavailable
            | SponsorshipError::CredentialUnavailable
            | SponsorshipError::ProviderPolicyDenied
            | SponsorshipError::TermsNotAccepted
            | SponsorshipError::TermsChanged
            | SponsorshipError::ScopeDenied
            | SponsorshipError::GrantUnavailable
            | SponsorshipError::Store(SponsorshipStoreError::GrantUnavailable)
            | SponsorshipError::Store(SponsorshipStoreError::LaunchAuthorityChanged)
            | SponsorshipError::Store(SponsorshipStoreError::AcceptanceChanged)
    )
}

fn lower_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
        time::Duration,
    };

    use super::*;
    use crate::{
        LaunchId, SensitiveProviderValue, SessionId, SponsoredGrantId, SponsoredPriceQuote,
        SponsoredUsageSummary,
    };
    use tokio::sync::Notify;

    fn request_with_order(request: Value) -> SponsoredModelRequest {
        let canonical = canonical_json_bytes(&request).unwrap();
        let digest = lower_hex(Sha256::digest(canonical));
        SponsoredModelRequest {
            stable_request_id: format!("smr-b-{}-{digest}", "a".repeat(64)),
            request_digest: digest,
            maximum_quota: SponsoredQuota {
                requests: 1,
                input_tokens: canonical_json_bytes(&request).unwrap().len() as u64,
                output_tokens: 10,
            },
            model: "model-a".into(),
            resource: layer3_adapter::SPONSORED_MODEL_RESOURCE.into(),
            request,
        }
    }

    #[test]
    fn request_validation_binds_canonical_json_and_exact_quota() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "input": {"b": 2, "a": 1},
            "model": "model-a",
        }));
        request.validate().unwrap();

        let mut changed = request.clone();
        changed.maximum_quota.output_tokens += 1;
        assert!(changed.validate().is_err());
        let mut streaming = request;
        streaming.request["stream"] = Value::Bool(true);
        assert!(streaming.validate().is_err());
    }

    #[test]
    fn responses_is_the_canonical_policy_resource_and_input_bytes_are_conservative() {
        let request = request_with_order(serde_json::json!({
            "input": "one input token is represented by several JSON bytes",
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        assert_eq!(request.resource, layer3_adapter::SPONSORED_MODEL_RESOURCE);
        assert_eq!(request.resource, "responses");
        assert_eq!(
            request.maximum_quota.input_tokens,
            canonical_json_bytes(&request.request).unwrap().len() as u64
        );
        assert!(request.maximum_quota.input_tokens >= 10);
        request.validate().unwrap();
    }

    #[test]
    fn response_custody_rejects_payloads_above_two_mebibytes() {
        let oversized = serde_json::json!({
            "output": "x".repeat(MAX_SPONSORED_MODEL_RESPONSE_BYTES + 1),
        });
        assert!(SponsoredModelResponsePayload::from_response(
            "a".repeat(64),
            SponsoredQuota {
                requests: 1,
                input_tokens: 1,
                output_tokens: 1,
            },
            &oversized,
        )
        .is_err());
    }

    struct FakeAccounting {
        prepared: Mutex<Option<PreparedOperation>>,
        events: Arc<Mutex<Vec<&'static str>>>,
        fences: Arc<Mutex<Vec<Option<SponsoredUsageReconciliationFence>>>>,
        fail_reconcile: bool,
        claim_behavior: FakeClaimBehavior,
    }

    #[derive(Clone, Copy)]
    enum FakeClaimBehavior {
        Acquire,
        KillSwitch,
        InProgress,
    }

    type ServiceFixture = (
        SponsoredModelDataPlaneService,
        Arc<Mutex<Vec<&'static str>>>,
        Arc<FakeProvider>,
        Arc<Mutex<Vec<Option<SponsoredUsageReconciliationFence>>>>,
    );

    #[async_trait]
    impl SponsoredModelAccounting for FakeAccounting {
        async fn prepare(
            &self,
            _capability: &InertGrantCapability,
            _request: &SponsoredModelRequest,
        ) -> Result<PreparedModelOperation, SponsorshipError> {
            self.events.lock().unwrap().push("reserve");
            self.prepared
                .lock()
                .unwrap()
                .take()
                .map(|operation| PreparedModelOperation::Operation(Box::new(operation)))
                .ok_or_else(|| {
                    SponsorshipStoreError::Malformed(
                        "fixture operation was already consumed".into(),
                    )
                    .into()
                })
        }

        async fn claim(
            &self,
            _capability: &InertGrantCapability,
            reservation: &SponsoredUsageReservation,
        ) -> Result<SponsoredUsageReservation, SponsorshipError> {
            self.events.lock().unwrap().push("claim");
            match self.claim_behavior {
                FakeClaimBehavior::Acquire => {}
                FakeClaimBehavior::KillSwitch => return Err(SponsorshipError::KillSwitch),
                FakeClaimBehavior::InProgress => {
                    return Err(SponsorshipError::ProviderOperationInProgress);
                }
            }
            let mut claimed = reservation.clone();
            claimed.state = SponsoredUsageState::Attempting;
            claimed.dispatch_revision += 1;
            claimed.dispatch_lease = Some(SponsoredUsageDispatchLease {
                holder: "44444444-4444-4444-8444-444444444444".into(),
                acquired_at: "2026-01-02T03:04:06Z".into(),
                expires_at: "2026-01-02T03:04:36Z".into(),
            });
            claimed.updated_at = "2026-01-02T03:04:06Z".into();
            Ok(claimed)
        }

        async fn reconcile(
            &self,
            reservation: &SponsoredUsageReservation,
            outcome: SponsoredUsageState,
            actual_quota: Option<SponsoredQuota>,
            dispatch_fence: Option<SponsoredUsageReconciliationFence>,
            _model_response: Option<SponsoredModelResponsePayload>,
        ) -> Result<SponsoredUsageReservation, SponsorshipError> {
            self.fences.lock().unwrap().push(dispatch_fence);
            self.events.lock().unwrap().push(match outcome {
                SponsoredUsageState::Settled => "settle",
                SponsoredUsageState::Released => "release",
                SponsoredUsageState::Ambiguous => "ambiguous",
                SponsoredUsageState::Reserved | SponsoredUsageState::Attempting => "invalid",
            });
            if self.fail_reconcile {
                return Err(SponsorshipError::Disabled);
            }
            let mut resolved = reservation.clone();
            resolved.state = outcome;
            resolved.dispatch_lease = None;
            resolved.actual = actual_quota.map(|quota| SponsoredUsageSummary {
                quota,
                spend_micros: 1,
            });
            Ok(resolved)
        }
    }

    struct FakeProvider {
        execution: Mutex<Option<SponsoredModelProviderExecution>>,
        observation: Mutex<Option<SponsoredModelProviderObservation>>,
        events: Arc<Mutex<Vec<&'static str>>>,
        operations: Mutex<Vec<SponsoredUsageReservationId>>,
    }

    #[async_trait]
    impl SponsoredModelProvider for FakeProvider {
        async fn execute(
            &self,
            _provider: &str,
            _authority: &SponsoredModelProviderAuthority,
            request: &SponsoredModelProviderRequest,
        ) -> SponsoredModelProviderExecution {
            self.events.lock().unwrap().push("execute");
            self.operations
                .lock()
                .unwrap()
                .push(request.operation_id.clone());
            self.execution.lock().unwrap().take().unwrap()
        }

        async fn observe(
            &self,
            _provider: &str,
            _authority: &SponsoredModelProviderAuthority,
            request: &SponsoredModelProviderRequest,
        ) -> SponsoredModelProviderObservation {
            self.events.lock().unwrap().push("observe");
            self.operations
                .lock()
                .unwrap()
                .push(request.operation_id.clone());
            self.observation.lock().unwrap().take().unwrap()
        }
    }

    fn capability() -> InertGrantCapability {
        InertGrantCapability {
            grant_id: SponsoredGrantId::parse("22222222-2222-4222-8222-222222222222").unwrap(),
            grant_generation: 1,
            policy_generation: 1,
            launch_id: LaunchId::parse("11111111-1111-4111-8111-111111111111").unwrap(),
            session_id: SessionId("sesn_trg_123456781234123412341234567890ab".into()),
            expires_at: "2099-01-02T03:04:05Z".into(),
            fingerprint: "f".repeat(64),
        }
    }

    fn response() -> SponsoredModelProviderResponse {
        SponsoredModelProviderResponse {
            response: serde_json::json!({"id": "response-1", "object": "response"}),
            actual_quota: SponsoredQuota {
                requests: 1,
                input_tokens: 4,
                output_tokens: 5,
            },
        }
    }

    fn prepared(
        request: &SponsoredModelRequest,
        disposition: ReservationDisposition,
        state: SponsoredUsageState,
    ) -> PreparedOperation {
        let maximum = SponsoredUsageSummary {
            quota: request.maximum_quota,
            spend_micros: 1_000,
        };
        let mut quote = SponsoredPriceQuote {
            provider: "provider-a".into(),
            model: request.model.clone(),
            resource: request.resource.clone(),
            maximum: maximum.clone(),
            pricing_generation: 1,
            digest: String::new(),
        };
        quote.digest = quote.calculate_digest().unwrap();
        PreparedOperation {
            provider: "provider-a".into(),
            authority: SponsoredModelProviderAuthority {
                root_credential: ProviderCredentialSecret::new("root-provider-secret").unwrap(),
                grant_receipt: ProviderGrantReceipt {
                    provider_grant_id: SensitiveProviderValue::new("provider-grant").unwrap(),
                    provider_state_version: 1,
                    expires_at: "2099-01-02T03:04:05Z".into(),
                    revocation_handle: SensitiveProviderValue::new("revocation").unwrap(),
                    issued_at: "2026-01-02T03:04:05Z".into(),
                },
            },
            capability: capability(),
            request_digest: request.request_digest.clone(),
            disposition,
            reservation: SponsoredUsageReservation {
                id: SponsoredUsageReservationId::parse("33333333-3333-4333-8333-333333333333")
                    .unwrap(),
                idempotency_fingerprint: "a".repeat(64),
                request_fingerprint: "b".repeat(64),
                grant_id: capability().grant_id,
                price_quote: quote,
                reserved: maximum,
                state,
                dispatch_revision: u64::from(state != SponsoredUsageState::Reserved),
                dispatch_lease: (state == SponsoredUsageState::Attempting).then(|| {
                    SponsoredUsageDispatchLease {
                        holder: "44444444-4444-4444-8444-444444444444".into(),
                        acquired_at: "2026-01-02T03:04:05Z".into(),
                        expires_at: "2026-01-02T03:04:35Z".into(),
                    }
                }),
                created_at: "2026-01-02T03:04:05Z".into(),
                updated_at: "2026-01-02T03:04:05Z".into(),
                actual: None,
            },
        }
    }

    fn service(
        request: &SponsoredModelRequest,
        disposition: ReservationDisposition,
        state: SponsoredUsageState,
        execution: SponsoredModelProviderExecution,
        observation: SponsoredModelProviderObservation,
        fail_reconcile: bool,
        claim_behavior: FakeClaimBehavior,
    ) -> ServiceFixture {
        let events = Arc::new(Mutex::new(Vec::new()));
        let fences = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(FakeProvider {
            execution: Mutex::new(Some(execution)),
            observation: Mutex::new(Some(observation)),
            events: Arc::clone(&events),
            operations: Mutex::new(Vec::new()),
        });
        let accounting = Arc::new(FakeAccounting {
            prepared: Mutex::new(Some(prepared(request, disposition, state))),
            events: Arc::clone(&events),
            fences: Arc::clone(&fences),
            fail_reconcile,
            claim_behavior,
        });
        (
            SponsoredModelDataPlaneService {
                accounting,
                provider: provider.clone(),
            },
            events,
            provider,
            fences,
        )
    }

    struct LinearizableTombstoneAccounting {
        reservation: Mutex<SponsoredUsageReservation>,
        prepare_calls: AtomicUsize,
        block_claim_before_provider: AtomicBool,
        claim_persisted: Notify,
        release_claim: Notify,
    }

    impl LinearizableTombstoneAccounting {
        fn new(request: &SponsoredModelRequest, block_claim_before_provider: bool) -> Self {
            Self {
                reservation: Mutex::new(
                    prepared(
                        request,
                        ReservationDisposition::Created,
                        SponsoredUsageState::Reserved,
                    )
                    .reservation,
                ),
                prepare_calls: AtomicUsize::new(0),
                block_claim_before_provider: AtomicBool::new(block_claim_before_provider),
                claim_persisted: Notify::new(),
                release_claim: Notify::new(),
            }
        }
    }

    #[async_trait]
    impl SponsoredModelAccounting for LinearizableTombstoneAccounting {
        async fn prepare(
            &self,
            _capability: &InertGrantCapability,
            request: &SponsoredModelRequest,
        ) -> Result<PreparedModelOperation, SponsorshipError> {
            let disposition = if self.prepare_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ReservationDisposition::Created
            } else {
                ReservationDisposition::Existing
            };
            let reservation = self.reservation.lock().unwrap().clone();
            let mut operation = prepared(request, disposition, reservation.state);
            operation.reservation = reservation;
            Ok(PreparedModelOperation::Operation(Box::new(operation)))
        }

        async fn claim(
            &self,
            _capability: &InertGrantCapability,
            reservation: &SponsoredUsageReservation,
        ) -> Result<SponsoredUsageReservation, SponsorshipError> {
            let claimed = {
                let mut stored = self.reservation.lock().unwrap();
                if stored.id != reservation.id || stored.state != SponsoredUsageState::Reserved {
                    return Err(SponsorshipError::ProviderOperationInProgress);
                }
                stored.state = SponsoredUsageState::Attempting;
                stored.dispatch_revision = 1;
                stored.dispatch_lease = Some(SponsoredUsageDispatchLease {
                    holder: "44444444-4444-4444-8444-444444444444".into(),
                    acquired_at: "2026-01-02T03:04:06Z".into(),
                    expires_at: "2026-01-02T03:04:36Z".into(),
                });
                stored.updated_at = "2026-01-02T03:04:06Z".into();
                stored.clone()
            };
            if self
                .block_claim_before_provider
                .swap(false, Ordering::SeqCst)
            {
                self.claim_persisted.notify_one();
                self.release_claim.notified().await;
            }
            Ok(claimed)
        }

        async fn reconcile(
            &self,
            reservation: &SponsoredUsageReservation,
            outcome: SponsoredUsageState,
            actual_quota: Option<SponsoredQuota>,
            _dispatch_fence: Option<SponsoredUsageReconciliationFence>,
            model_response: Option<SponsoredModelResponsePayload>,
        ) -> Result<SponsoredUsageReservation, SponsorshipError> {
            if outcome != SponsoredUsageState::Released
                || actual_quota.is_some()
                || model_response.is_some()
            {
                return Err(SponsorshipStoreError::Conflict.into());
            }
            let mut stored = self.reservation.lock().unwrap();
            if stored.id != reservation.id {
                return Err(SponsorshipStoreError::Conflict.into());
            }
            stored.state = SponsoredUsageState::Released;
            stored.dispatch_lease = None;
            stored.actual = None;
            Ok(stored.clone())
        }
    }

    struct LinearizableTombstoneProvider {
        execute_started: Notify,
        tombstone_installed: Notify,
        tombstoned: AtomicBool,
        effects: AtomicUsize,
        executions: AtomicUsize,
        observations: AtomicUsize,
    }

    impl LinearizableTombstoneProvider {
        fn new() -> Self {
            Self {
                execute_started: Notify::new(),
                tombstone_installed: Notify::new(),
                tombstoned: AtomicBool::new(false),
                effects: AtomicUsize::new(0),
                executions: AtomicUsize::new(0),
                observations: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl SponsoredModelProvider for LinearizableTombstoneProvider {
        async fn execute(
            &self,
            _provider: &str,
            _authority: &SponsoredModelProviderAuthority,
            _request: &SponsoredModelProviderRequest,
        ) -> SponsoredModelProviderExecution {
            self.executions.fetch_add(1, Ordering::SeqCst);
            let tombstone_installed = self.tombstone_installed.notified();
            self.execute_started.notify_one();
            tombstone_installed.await;
            if self.tombstoned.load(Ordering::SeqCst) {
                SponsoredModelProviderExecution::Rejected
            } else {
                self.effects.fetch_add(1, Ordering::SeqCst);
                SponsoredModelProviderExecution::Confirmed(response())
            }
        }

        async fn observe(
            &self,
            _provider: &str,
            _authority: &SponsoredModelProviderAuthority,
            _request: &SponsoredModelProviderRequest,
        ) -> SponsoredModelProviderObservation {
            self.observations.fetch_add(1, Ordering::SeqCst);
            self.tombstoned.store(true, Ordering::SeqCst);
            self.tombstone_installed.notify_one();
            SponsoredModelProviderObservation::Rejected
        }
    }

    #[tokio::test]
    async fn rejected_observation_fences_an_older_in_flight_execution_before_release() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let accounting = Arc::new(LinearizableTombstoneAccounting::new(&request, false));
        let provider = Arc::new(LinearizableTombstoneProvider::new());
        let service = Arc::new(SponsoredModelDataPlaneService {
            accounting,
            provider: provider.clone(),
        });

        let first = tokio::spawn({
            let service = service.clone();
            let request = request.clone();
            async move { service.execute(&capability(), request).await }
        });
        tokio::time::timeout(Duration::from_secs(2), provider.execute_started.notified())
            .await
            .expect("the older provider execution starts");

        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::ProviderRejected)
        ));
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), first)
                .await
                .expect("the fenced execution completes")
                .expect("the execution task does not panic"),
            Err(SponsoredModelDataPlaneError::ProviderRejected)
        ));
        assert!(provider.tombstoned.load(Ordering::SeqCst));
        assert_eq!(provider.executions.load(Ordering::SeqCst), 1);
        assert_eq!(provider.observations.load(Ordering::SeqCst), 1);
        assert_eq!(provider.effects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rejected_observation_fences_a_stale_holder_before_its_provider_poll() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let accounting = Arc::new(LinearizableTombstoneAccounting::new(&request, true));
        let provider = Arc::new(LinearizableTombstoneProvider::new());
        let service = Arc::new(SponsoredModelDataPlaneService {
            accounting: accounting.clone(),
            provider: provider.clone(),
        });

        let stale_holder = tokio::spawn({
            let service = service.clone();
            let request = request.clone();
            async move { service.execute(&capability(), request).await }
        });
        tokio::time::timeout(
            Duration::from_secs(2),
            accounting.claim_persisted.notified(),
        )
        .await
        .expect("the stale holder durably claims before its provider call");

        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::ProviderRejected)
        ));
        assert_eq!(provider.observations.load(Ordering::SeqCst), 1);
        assert_eq!(provider.executions.load(Ordering::SeqCst), 0);
        accounting.release_claim.notify_one();

        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), stale_holder)
                .await
                .expect("the stale holder reaches the provider tombstone")
                .expect("the stale holder task does not panic"),
            Err(SponsoredModelDataPlaneError::ProviderRejected)
        ));
        assert!(provider.tombstoned.load(Ordering::SeqCst));
        assert_eq!(provider.executions.load(Ordering::SeqCst), 1);
        assert_eq!(provider.observations.load(Ordering::SeqCst), 1);
        assert_eq!(provider.effects.load(Ordering::SeqCst), 0);
        assert_eq!(
            accounting.reservation.lock().unwrap().state,
            SponsoredUsageState::Released
        );
    }

    #[tokio::test]
    async fn reservation_precedes_dispatch_and_settlement_precedes_response() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, _, _) = service(
            &request,
            ReservationDisposition::Created,
            SponsoredUsageState::Reserved,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::Pending,
            false,
            FakeClaimBehavior::Acquire,
        );
        let confirmed = service.execute(&capability(), request).await.unwrap();
        assert_eq!(confirmed.response["id"], "response-1");
        assert_eq!(
            *events.lock().unwrap(),
            ["reserve", "claim", "execute", "settle"]
        );
    }

    #[tokio::test]
    async fn existing_reservation_is_observed_before_same_operation_retry() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, provider, _) = service(
            &request,
            ReservationDisposition::Existing,
            SponsoredUsageState::Ambiguous,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::NotFound,
            false,
            FakeClaimBehavior::Acquire,
        );
        service.execute(&capability(), request).await.unwrap();
        assert_eq!(
            *events.lock().unwrap(),
            ["reserve", "observe", "claim", "execute", "settle"]
        );
        let operations = provider.operations.lock().unwrap();
        assert_eq!(operations.len(), 2);
        assert_eq!(operations[0], operations[1]);
    }

    #[tokio::test]
    async fn existing_absent_operation_cannot_cross_a_closed_gate() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, provider, fences) = service(
            &request,
            ReservationDisposition::Existing,
            SponsoredUsageState::Reserved,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::NotFound,
            false,
            FakeClaimBehavior::KillSwitch,
        );
        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::ReservationReleased)
        ));
        assert_eq!(
            *events.lock().unwrap(),
            ["reserve", "observe", "claim", "release"]
        );
        assert_eq!(provider.operations.lock().unwrap().len(), 1);
        assert_eq!(
            *fences.lock().unwrap(),
            [Some(SponsoredUsageReconciliationFence::NeverDispatched)]
        );
    }

    #[tokio::test]
    async fn refused_first_claim_releases_only_never_dispatched_reservation() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, provider, fences) = service(
            &request,
            ReservationDisposition::Created,
            SponsoredUsageState::Reserved,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::Pending,
            false,
            FakeClaimBehavior::KillSwitch,
        );

        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::Sponsorship(
                SponsorshipError::KillSwitch
            ))
        ));
        assert_eq!(*events.lock().unwrap(), ["reserve", "claim", "release"]);
        assert!(provider.operations.lock().unwrap().is_empty());
        assert_eq!(
            *fences.lock().unwrap(),
            [Some(SponsoredUsageReconciliationFence::NeverDispatched)]
        );
    }

    #[tokio::test]
    async fn pending_observation_is_read_only_even_with_a_live_attempt_lease() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, provider, fences) = service(
            &request,
            ReservationDisposition::Existing,
            SponsoredUsageState::Attempting,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::Pending,
            false,
            FakeClaimBehavior::Acquire,
        );

        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::ProviderAmbiguous)
        ));
        assert_eq!(*events.lock().unwrap(), ["reserve", "observe"]);
        assert_eq!(provider.operations.lock().unwrap().len(), 1);
        assert!(fences.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn not_found_with_a_live_claim_neither_dispatches_nor_releases() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, provider, fences) = service(
            &request,
            ReservationDisposition::Existing,
            SponsoredUsageState::Attempting,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::NotFound,
            false,
            FakeClaimBehavior::InProgress,
        );

        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::ProviderAmbiguous)
        ));
        assert_eq!(*events.lock().unwrap(), ["reserve", "observe", "claim"]);
        assert_eq!(provider.operations.lock().unwrap().len(), 1);
        assert!(fences.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn positive_revision_not_found_and_closed_gate_retains_custody_and_hold() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, provider, fences) = service(
            &request,
            ReservationDisposition::Existing,
            SponsoredUsageState::Ambiguous,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::NotFound,
            false,
            FakeClaimBehavior::KillSwitch,
        );

        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::ProviderAmbiguous)
        ));
        assert_eq!(*events.lock().unwrap(), ["reserve", "observe", "claim"]);
        assert_eq!(provider.operations.lock().unwrap().len(), 1);
        assert!(fences.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pre_dispatch_unavailability_releases_revision_one_but_not_later_claims() {
        for (disposition, state, observation, expected_event, expected_error, revision) in [
            (
                ReservationDisposition::Created,
                SponsoredUsageState::Reserved,
                SponsoredModelProviderObservation::Pending,
                "release",
                "unavailable",
                1,
            ),
            (
                ReservationDisposition::Existing,
                SponsoredUsageState::Ambiguous,
                SponsoredModelProviderObservation::NotFound,
                "ambiguous",
                "ambiguous",
                2,
            ),
        ] {
            let request = request_with_order(serde_json::json!({
                "max_output_tokens": 10,
                "model": "model-a",
            }));
            let (service, events, _, fences) = service(
                &request,
                disposition,
                state,
                SponsoredModelProviderExecution::UnavailableBeforeDispatch,
                observation,
                false,
                FakeClaimBehavior::Acquire,
            );
            let error = service.execute(&capability(), request).await.unwrap_err();
            if expected_error == "unavailable" {
                assert!(matches!(
                    error,
                    SponsoredModelDataPlaneError::ProviderUnavailable
                ));
                assert_eq!(
                    *events.lock().unwrap(),
                    ["reserve", "claim", "execute", expected_event]
                );
            } else {
                assert!(matches!(
                    error,
                    SponsoredModelDataPlaneError::ProviderAmbiguous
                ));
                assert_eq!(
                    *events.lock().unwrap(),
                    ["reserve", "observe", "claim", "execute", expected_event]
                );
            }
            assert_eq!(
                *fences.lock().unwrap(),
                [Some(SponsoredUsageReconciliationFence::DispatchClaim {
                    revision,
                    holder: "44444444-4444-4444-8444-444444444444".into(),
                })]
            );
        }
    }

    #[tokio::test]
    async fn definitive_observations_reconcile_the_exact_state_and_revision() {
        for (observation, terminal_event) in [
            (
                SponsoredModelProviderObservation::Confirmed(response()),
                "settle",
            ),
            (SponsoredModelProviderObservation::Rejected, "release"),
        ] {
            let request = request_with_order(serde_json::json!({
                "max_output_tokens": 10,
                "model": "model-a",
            }));
            let (service, events, _, fences) = service(
                &request,
                ReservationDisposition::Existing,
                SponsoredUsageState::Ambiguous,
                SponsoredModelProviderExecution::Confirmed(response()),
                observation,
                false,
                FakeClaimBehavior::Acquire,
            );

            let result = service.execute(&capability(), request).await;
            if terminal_event == "settle" {
                result.unwrap();
            } else {
                assert!(matches!(
                    result,
                    Err(SponsoredModelDataPlaneError::ProviderRejected)
                ));
            }
            assert_eq!(
                *events.lock().unwrap(),
                ["reserve", "observe", terminal_event]
            );
            assert_eq!(
                *fences.lock().unwrap(),
                [Some(
                    SponsoredUsageReconciliationFence::ProviderObservation {
                        state: SponsoredUsageState::Ambiguous,
                        revision: 1,
                    }
                )]
            );
        }
    }

    #[tokio::test]
    async fn definitive_and_uncertain_failures_record_their_exact_terminal_state() {
        let cases = [
            (
                SponsoredModelProviderExecution::Rejected,
                "release",
                "rejected",
            ),
            (
                SponsoredModelProviderExecution::UnavailableBeforeDispatch,
                "release",
                "unavailable",
            ),
            (
                SponsoredModelProviderExecution::Ambiguous,
                "ambiguous",
                "ambiguous",
            ),
        ];
        for (execution, terminal_event, expected_error) in cases {
            let request = request_with_order(serde_json::json!({
                "max_output_tokens": 10,
                "model": "model-a",
            }));
            let (service, events, _, _) = service(
                &request,
                ReservationDisposition::Created,
                SponsoredUsageState::Reserved,
                execution,
                SponsoredModelProviderObservation::Pending,
                false,
                FakeClaimBehavior::Acquire,
            );
            let error = service.execute(&capability(), request).await.unwrap_err();
            assert!(match expected_error {
                "rejected" => matches!(error, SponsoredModelDataPlaneError::ProviderRejected),
                "unavailable" => {
                    matches!(error, SponsoredModelDataPlaneError::ProviderUnavailable)
                }
                _ => matches!(error, SponsoredModelDataPlaneError::ProviderAmbiguous),
            });
            assert_eq!(
                *events.lock().unwrap(),
                ["reserve", "claim", "execute", terminal_event]
            );
        }
    }

    #[tokio::test]
    async fn provider_response_is_withheld_when_settlement_fails() {
        let request = request_with_order(serde_json::json!({
            "max_output_tokens": 10,
            "model": "model-a",
        }));
        let (service, events, _, _) = service(
            &request,
            ReservationDisposition::Created,
            SponsoredUsageState::Reserved,
            SponsoredModelProviderExecution::Confirmed(response()),
            SponsoredModelProviderObservation::Pending,
            true,
            FakeClaimBehavior::Acquire,
        );
        assert!(matches!(
            service.execute(&capability(), request).await,
            Err(SponsoredModelDataPlaneError::Accounting(_))
        ));
        assert_eq!(
            *events.lock().unwrap(),
            ["reserve", "claim", "execute", "settle"]
        );
    }

    #[tokio::test]
    async fn provider_response_reflecting_protected_authority_is_marked_ambiguous_and_withheld() {
        for reflected in ["root-provider-secret", "provider-grant", "revocation"] {
            let request = request_with_order(serde_json::json!({
                "max_output_tokens": 10,
                "model": "model-a",
            }));
            let mut reflected_response = response();
            reflected_response.response["reflected"] = Value::String(reflected.into());
            let (service, events, _, _) = service(
                &request,
                ReservationDisposition::Created,
                SponsoredUsageState::Reserved,
                SponsoredModelProviderExecution::Confirmed(reflected_response),
                SponsoredModelProviderObservation::Pending,
                false,
                FakeClaimBehavior::Acquire,
            );
            assert!(matches!(
                service.execute(&capability(), request).await,
                Err(SponsoredModelDataPlaneError::ProviderAmbiguous)
            ));
            assert_eq!(
                *events.lock().unwrap(),
                ["reserve", "claim", "execute", "ambiguous"]
            );
        }
    }
}
