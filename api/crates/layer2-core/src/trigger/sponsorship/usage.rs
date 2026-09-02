//! Atomic sponsored usage reservation and trusted reconciliation.

use super::super::TriggerValidationError;
use super::lifecycle::{
    monotonic_record_time, ReportSponsoredUsage, RequestSponsoredUsage, SponsorshipError,
    SponsorshipService,
};
use super::model::*;
use super::ports::SponsorshipStoreError;
use super::validation::*;
use crate::{Caller, Capability};

impl SponsorshipService {
    pub async fn reserve_usage(
        &self,
        caller: &Caller,
        command: RequestSponsoredUsage,
    ) -> Result<SponsoredUsageReservationOutcome, SponsorshipError> {
        self.reserve_usage_inner(caller, command, None).await
    }

    /// Reserve one model request while binding its durable replay identity to
    /// the canonical request digest. This remains crate-private because the
    /// generic public usage command deliberately does not carry model payloads.
    pub(crate) async fn reserve_model_usage(
        &self,
        caller: &Caller,
        command: RequestSponsoredUsage,
        model_dispatch: SponsoredModelDispatchPayload,
    ) -> Result<SponsoredUsageReservationOutcome, SponsorshipError> {
        model_dispatch.validate()?;
        self.reserve_usage_inner(caller, command, Some(model_dispatch))
            .await
    }

    async fn reserve_usage_inner(
        &self,
        caller: &Caller,
        command: RequestSponsoredUsage,
        model_dispatch: Option<SponsoredModelDispatchPayload>,
    ) -> Result<SponsoredUsageReservationOutcome, SponsorshipError> {
        require_contributor_capability(caller)?;
        validate_stable_request_id(&command.stable_request_id)?;
        validate_policy_token("usage.provider", &command.provider)?;
        validate_policy_token("usage.model", &command.model)?;
        validate_policy_token("usage.resource", &command.resource)?;
        if !command.maximum_quota.is_nonzero() {
            return Err(TriggerValidationError::field(
                "usage.maximum_quota",
                "must reserve positive request, input-token, and output-token bounds",
            )
            .into());
        }
        let idempotency_fingerprint = match model_dispatch.as_ref() {
            Some(model_dispatch) => model_usage_idempotency_fingerprint(
                &caller.subject,
                &command,
                model_dispatch.request_digest(),
            )?,
            None => usage_idempotency_fingerprint(&caller.subject, &command)?,
        };
        let request_identity = self.operational.pseudonymize(
            SponsorshipOperationalDomain::UsageRequest,
            &[
                &command.grant_id.0,
                &caller.subject.0,
                &command.stable_request_id,
            ],
        )?;
        if let Some(existing) = self
            .store
            .usage_reservation_for_request(&request_identity)
            .await?
        {
            existing.validate()?;
            if existing.idempotency_fingerprint != idempotency_fingerprint {
                return Err(SponsorshipStoreError::Conflict.into());
            }
            return Ok(SponsoredUsageReservationOutcome {
                disposition: ReservationDisposition::Existing,
                reservation: existing,
            });
        }
        let gate_fence = self.require_enabled().await?;
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let grant = self
            .store
            .grant(&command.grant_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        grant.validate()?;
        if grant.contributor != caller.subject
            || command.capability != grant.capability
            || !grant.is_active_at(&now)
        {
            return Err(SponsorshipError::GrantUnavailable);
        }
        let launch = self
            .store
            .launch_authority(&grant.launch_id)
            .await?
            .ok_or(SponsorshipError::GrantUnavailable)?;
        launch.validate()?;
        let session_id = launch.require_spend()?;
        if launch.contributor != caller.subject
            || launch.tenant_id != grant.tenant_id
            || launch.launch_id != grant.launch_id
            || session_id != &grant.session_id
            || launch.revision_id != grant.trigger_revision_id
            || launch.revision_digest != grant.trigger_revision_digest
            || launch.policy_id != grant.policy_id
        {
            return Err(SponsorshipError::ScopeDenied);
        }
        let policy = self.store.policy(&grant.policy_id).await?.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "usage grant references a missing sponsorship policy".into(),
            )
        })?;
        require_stored_grant_policy_bindings(&grant, &policy)?;
        self.gate
            .require_trusted_provider_policy_key(&policy.provider_policy.provider_policy_key)?;
        let stored_credential = self
            .store
            .credential_record(&policy.credential_id)
            .await?
            .ok_or(SponsorshipError::CredentialUnavailable)?;
        let credential = stored_credential;
        if !policy.is_available_at(&now)
            || credential.state != SponsoredCredentialState::Active
            || grant.policy_generation != policy.generation
        {
            return Err(SponsorshipError::GrantUnavailable);
        }
        if command.provider != policy.provider_policy.provider
            || !policy
                .provider_policy
                .allowed_models
                .contains(&command.model)
            || !policy
                .provider_policy
                .allowed_resources
                .contains(&command.resource)
        {
            return Err(SponsorshipError::ScopeDenied);
        }
        let quote = self
            .pricing
            .quote(
                &command.provider,
                &command.model,
                &command.resource,
                command.maximum_quota,
                &now,
            )
            .await
            .map_err(map_pricing_error)?;
        quote.validate()?;
        if quote.provider != command.provider
            || quote.model != command.model
            || quote.resource != command.resource
            || quote.maximum.quota != command.maximum_quota
        {
            return Err(SponsorshipError::PricingUnsupported);
        }
        let stable_request_id = command.stable_request_id;
        let request = SponsoredUsageRequest {
            reservation_id: SponsoredUsageReservationId::new(),
            request_identity,
            idempotency_fingerprint,
            grant_id: grant.id.clone(),
            requester: caller.subject.clone(),
            tenant_id: grant.tenant_id.clone(),
            trigger_revision_id: grant.trigger_revision_id.clone(),
            trigger_revision_digest: grant.trigger_revision_digest.clone(),
            launch_id: grant.launch_id.clone(),
            session_id: grant.session_id.clone(),
            provider: command.provider,
            model: command.model,
            resource: command.resource,
            capability_fingerprint: grant.capability.fingerprint.clone(),
            maximum: quote.maximum.clone(),
            price_quote_digest: quote.digest.clone(),
            requested_at: now.clone(),
        };
        request.validate()?;
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::UsageReserved,
            &grant.contributor,
            SponsoredAuditTarget::Grant {
                policy_id: policy.id,
                grant_id: grant.id.clone(),
            },
            Some(&stable_request_id),
            "reserved",
            now.clone(),
        );
        Ok(self
            .store
            .reserve_usage(
                ReserveSponsoredUsage {
                    request,
                    price_quote: quote,
                    launch_authority: launch,
                    gate_fence,
                    policy_generation: policy.generation,
                    credential_generation: grant.credential_generation,
                    model_dispatch,
                    now,
                },
                audit,
            )
            .await?)
    }

    /// Cross the provider-dispatch boundary under one durable, database-time
    /// claim. Reading or reconciling a prior provider operation deliberately
    /// does not require this admission; the first execution and every
    /// authoritative `NotFound` retry do.
    pub(crate) async fn claim_model_usage_dispatch(
        &self,
        capability: &InertGrantCapability,
        reservation: &SponsoredUsageReservation,
    ) -> Result<SponsoredUsageReservation, SponsorshipError> {
        capability.validate()?;
        reservation.validate()?;
        if capability.grant_id != reservation.grant_id
            || !matches!(
                reservation.state,
                SponsoredUsageState::Reserved
                    | SponsoredUsageState::Attempting
                    | SponsoredUsageState::Ambiguous
            )
        {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        let gate_fence = self.require_enabled().await?;
        let grant = self
            .store
            .grant(&reservation.grant_id)
            .await?
            .ok_or(SponsorshipError::GrantUnavailable)?;
        if grant.capability != *capability {
            return Err(SponsorshipError::GrantUnavailable);
        }
        let holder = uuid::Uuid::new_v4().to_string();
        let expected_revision = reservation.dispatch_revision;
        let audit_time = monotonic_record_time(&reservation.updated_at, &self.clock.now());
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::UsageDispatchClaimed,
            &grant.contributor,
            SponsoredAuditTarget::Grant {
                policy_id: grant.policy_id.clone(),
                grant_id: grant.id.clone(),
            },
            Some(&reservation.id.0),
            "attempting",
            audit_time,
        );
        let claim = self
            .store
            .claim_usage_dispatch(ClaimSponsoredUsageDispatch {
                reservation_id: reservation.id.clone(),
                request_fingerprint: reservation.request_fingerprint.clone(),
                expected_state: reservation.state,
                expected_dispatch_revision: expected_revision,
                capability: capability.clone(),
                gate_fence,
                holder: holder.clone(),
                audit,
            })
            .await?;
        claim.reservation.validate()?;
        if !claim.acquired {
            return Err(SponsorshipError::ProviderOperationInProgress);
        }
        if claim.reservation.id != reservation.id
            || claim.reservation.request_fingerprint != reservation.request_fingerprint
            || claim.reservation.grant_id != reservation.grant_id
            || claim.reservation.state != SponsoredUsageState::Attempting
            || claim.reservation.dispatch_revision != expected_revision.saturating_add(1)
            || claim
                .reservation
                .dispatch_lease
                .as_ref()
                .is_none_or(|lease| lease.holder != holder)
        {
            return Err(SponsorshipStoreError::Malformed(
                "usage dispatch claim does not match its durable reservation".into(),
            )
            .into());
        }
        Ok(claim.reservation)
    }

    pub async fn reconcile_usage(
        &self,
        caller: &Caller,
        report: ReportSponsoredUsage,
    ) -> Result<SponsoredUsageReservation, SponsorshipError> {
        self.reconcile_usage_inner(caller, report, None, None).await
    }

    pub(crate) async fn reconcile_model_usage(
        &self,
        caller: &Caller,
        report: ReportSponsoredUsage,
        dispatch_fence: Option<SponsoredUsageReconciliationFence>,
        model_response: Option<SponsoredModelResponsePayload>,
    ) -> Result<SponsoredUsageReservation, SponsorshipError> {
        self.reconcile_usage_inner(caller, report, dispatch_fence, model_response)
            .await
    }

    async fn reconcile_usage_inner(
        &self,
        caller: &Caller,
        report: ReportSponsoredUsage,
        dispatch_fence: Option<SponsoredUsageReconciliationFence>,
        model_response: Option<SponsoredModelResponsePayload>,
    ) -> Result<SponsoredUsageReservation, SponsorshipError> {
        caller
            .authorization
            .require(Capability::ReconcileSponsoredUsage)
            .map_err(|_| SponsorshipError::Forbidden)?;
        SponsoredUsageReservationId::parse(report.reservation_id.0.clone())?;
        validate_digest("usage.request_fingerprint", &report.request_fingerprint)?;
        let reservation = self
            .store
            .usage_reservation(&report.reservation_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        reservation.validate()?;
        if reservation.request_fingerprint != report.request_fingerprint {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        if let Some(response) = model_response.as_ref() {
            response.validate()?;
            if report.outcome != SponsoredUsageState::Settled
                || report.actual_quota != Some(response.actual_quota())
            {
                return Err(SponsorshipStoreError::Conflict.into());
            }
        }
        if dispatch_fence.is_some()
            && report.outcome == SponsoredUsageState::Settled
            && model_response.is_none()
        {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        if report.outcome != SponsoredUsageState::Settled && model_response.is_some() {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        match dispatch_fence.as_ref() {
            Some(SponsoredUsageReconciliationFence::DispatchClaim { revision, holder }) => {
                if *revision == 0
                    || uuid::Uuid::parse_str(holder)
                        .ok()
                        .is_none_or(|parsed| parsed.to_string() != *holder)
                {
                    return Err(SponsorshipStoreError::Conflict.into());
                }
            }
            Some(SponsoredUsageReconciliationFence::ProviderObservation { state, .. }) => {
                if !matches!(
                    state,
                    SponsoredUsageState::Reserved
                        | SponsoredUsageState::Attempting
                        | SponsoredUsageState::Ambiguous
                ) || !matches!(
                    report.outcome,
                    SponsoredUsageState::Settled | SponsoredUsageState::Released
                ) {
                    return Err(SponsorshipStoreError::Conflict.into());
                }
            }
            Some(SponsoredUsageReconciliationFence::NeverDispatched)
                if report.outcome != SponsoredUsageState::Released =>
            {
                return Err(SponsorshipStoreError::Conflict.into());
            }
            Some(SponsoredUsageReconciliationFence::NeverDispatched) | None => {}
        }
        let terminal_replay_matches = match reservation.state {
            SponsoredUsageState::Settled => {
                report.outcome == SponsoredUsageState::Settled
                    && reservation.actual.as_ref().map(|actual| actual.quota) == report.actual_quota
            }
            SponsoredUsageState::Released => {
                report.outcome == SponsoredUsageState::Released && report.actual_quota.is_none()
            }
            SponsoredUsageState::Reserved
            | SponsoredUsageState::Attempting
            | SponsoredUsageState::Ambiguous => false,
        };
        if matches!(
            reservation.state,
            SponsoredUsageState::Settled | SponsoredUsageState::Released
        ) {
            if !terminal_replay_matches {
                return Err(SponsorshipStoreError::Conflict.into());
            }
            if let Some(candidate) = model_response.as_ref() {
                let stored = self
                    .store
                    .model_usage_settled_response(&reservation.id)
                    .await?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "settled model reservation has no response custody".into(),
                        )
                    })?;
                if &stored != candidate {
                    return Err(SponsorshipStoreError::Conflict.into());
                }
            }
            return Ok(reservation);
        }
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
        let transition_time = monotonic_record_time(&reservation.updated_at, &now);
        let actual = match report.outcome {
            SponsoredUsageState::Settled => {
                let actual_quota = report.actual_quota.ok_or_else(|| {
                    TriggerValidationError::field(
                        "usage.actual_quota",
                        "is required for a settled reservation",
                    )
                })?;
                if actual_quota.requests == 0 {
                    return Err(TriggerValidationError::field(
                        "usage.actual.requests",
                        "must account for the provider request",
                    )
                    .into());
                }
                let actual = self
                    .pricing
                    .price_actual(&reservation.price_quote, actual_quota, &now)
                    .await
                    .map_err(map_pricing_error)?;
                if actual.quota != actual_quota {
                    return Err(SponsorshipError::PricingUnsupported);
                }
                Some(actual)
            }
            SponsoredUsageState::Released | SponsoredUsageState::Ambiguous => {
                if report.actual_quota.is_some() {
                    return Err(TriggerValidationError::field(
                        "usage.actual_quota",
                        "must be absent until provider usage is confirmed",
                    )
                    .into());
                }
                None
            }
            SponsoredUsageState::Reserved | SponsoredUsageState::Attempting => {
                return Err(TriggerValidationError::field(
                    "usage.outcome",
                    "reconciliation cannot return a reservation to a dispatch state",
                )
                .into())
            }
        };
        if let Some(actual) = &actual {
            if !actual.fits_within(&reservation.reserved) {
                return Err(SponsorshipStoreError::Conflict.into());
            }
        }
        let command = ReconcileSponsoredUsage {
            reservation_id: report.reservation_id,
            request_fingerprint: report.request_fingerprint,
            actual_quota: report.actual_quota,
            actual,
            outcome: report.outcome,
            observed_at: transition_time.clone(),
            dispatch_fence,
            model_response,
        };
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::UsageReconciled,
            &caller.subject,
            SponsoredAuditTarget::Platform,
            Some(&command.reservation_id.0),
            match command.outcome {
                SponsoredUsageState::Settled => "settled",
                SponsoredUsageState::Released => "released",
                SponsoredUsageState::Ambiguous => "ambiguous",
                SponsoredUsageState::Reserved | SponsoredUsageState::Attempting => unreachable!(),
            },
            transition_time,
        );
        Ok(self.store.reconcile_usage(command, audit).await?)
    }
}
