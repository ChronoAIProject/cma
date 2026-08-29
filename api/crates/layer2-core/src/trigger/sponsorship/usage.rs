//! Atomic sponsored usage reservation and trusted reconciliation.

use super::super::TriggerValidationError;
use super::lifecycle::{
    ReportSponsoredUsage, RequestSponsoredUsage, SponsorshipError, SponsorshipService,
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
        let idempotency_fingerprint = usage_idempotency_fingerprint(&caller.subject, &command)?;
        if let Some(existing) = self
            .store
            .usage_reservation(&command.stable_request_id)
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
        let policy = self
            .store
            .policy(&grant.policy_id)
            .await?
            .ok_or(SponsorshipError::PolicyUnavailable)?;
        let stored_credential = self
            .store
            .credential(&policy.credential_id)
            .await?
            .ok_or(SponsorshipError::CredentialUnavailable)?;
        stored_credential.validate()?;
        let credential = stored_credential.record;
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
        let request = SponsoredUsageRequest {
            stable_request_id: command.stable_request_id,
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
        let stable_request_id = request.stable_request_id.clone();
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
                    now,
                },
                audit,
            )
            .await?)
    }

    pub async fn reconcile_usage(
        &self,
        caller: &Caller,
        report: ReportSponsoredUsage,
    ) -> Result<SponsoredUsageReservation, SponsorshipError> {
        caller
            .authorization
            .require(Capability::ReconcileSponsoredUsage)
            .map_err(|_| SponsorshipError::Forbidden)?;
        validate_stable_request_id(&report.stable_request_id)?;
        validate_digest("usage.request_fingerprint", &report.request_fingerprint)?;
        let reservation = self
            .store
            .usage_reservation(&report.stable_request_id)
            .await?
            .ok_or(SponsorshipError::NotFound)?;
        reservation.validate()?;
        if reservation.request_fingerprint != report.request_fingerprint {
            return Err(SponsorshipStoreError::Conflict.into());
        }
        let terminal_replay_matches = match reservation.state {
            SponsoredUsageState::Settled => {
                report.outcome == SponsoredUsageState::Settled
                    && reservation.actual.as_ref().map(|actual| actual.quota) == report.actual_quota
            }
            SponsoredUsageState::Released => {
                report.outcome == SponsoredUsageState::Released && report.actual_quota.is_none()
            }
            SponsoredUsageState::Reserved | SponsoredUsageState::Ambiguous => false,
        };
        if matches!(
            reservation.state,
            SponsoredUsageState::Settled | SponsoredUsageState::Released
        ) {
            return if terminal_replay_matches {
                Ok(reservation)
            } else {
                Err(SponsorshipStoreError::Conflict.into())
            };
        }
        let now = self.clock.now();
        validate_timestamp("now", &now)?;
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
            SponsoredUsageState::Reserved => {
                return Err(TriggerValidationError::field(
                    "usage.outcome",
                    "reconciliation cannot return a reservation to reserved",
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
            stable_request_id: report.stable_request_id,
            request_fingerprint: report.request_fingerprint,
            actual_quota: report.actual_quota,
            actual,
            outcome: report.outcome,
            observed_at: now.clone(),
        };
        let audit = SponsoredAuditEvent::privacy_safe(
            &self.audit,
            SponsoredAuditAction::UsageReconciled,
            &caller.subject,
            SponsoredAuditTarget::Platform,
            Some(&command.stable_request_id),
            match command.outcome {
                SponsoredUsageState::Settled => "settled",
                SponsoredUsageState::Released => "released",
                SponsoredUsageState::Ambiguous => "ambiguous",
                SponsoredUsageState::Reserved => unreachable!(),
            },
            now,
        );
        Ok(self.store.reconcile_usage(command, audit).await?)
    }
}
