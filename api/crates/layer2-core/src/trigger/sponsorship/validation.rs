//! Cross-cutting validation, deterministic identifiers, and error mapping.

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::TriggerValidationError;
use super::lifecycle::{RequestSponsoredUsage, SponsorshipError};
use super::model::*;
use super::ports::*;
use crate::{Caller, Capability, SubjectId};

pub(super) fn require_sponsor_caller(
    caller: &Caller,
    owner: &SponsorBillingOwner,
) -> Result<(), SponsorshipError> {
    if &caller.subject != owner.controller()
        || !caller
            .authorization
            .allows(Capability::ManageOwnSponsorships)
    {
        return Err(SponsorshipError::Forbidden);
    }
    Ok(())
}

pub(super) fn require_contributor_capability(caller: &Caller) -> Result<(), SponsorshipError> {
    caller
        .authorization
        .require(Capability::UseOwnSessions)
        .map_err(|_| SponsorshipError::Forbidden)
}

pub(super) fn usage_idempotency_fingerprint(
    requester: &SubjectId,
    command: &RequestSponsoredUsage,
) -> Result<String, TriggerValidationError> {
    command.capability.validate()?;
    canonical_digest(&(
        "cma-sponsored-usage-idempotency-v2",
        &command.grant_id,
        requester,
        &command.provider,
        &command.model,
        &command.resource,
        command.maximum_quota,
        &command.capability,
    ))
}

pub(super) fn require_grant_control(
    caller: &Caller,
    sponsor: &SponsorBillingOwner,
    contributor: &SubjectId,
) -> Result<(), SponsorshipError> {
    if &caller.subject == contributor && caller.authorization.allows(Capability::UseOwnSessions) {
        return Ok(());
    }
    require_sponsor_caller(caller, sponsor)
}

pub(super) fn require_acceptance_launch_bindings(
    acceptance: &SponsoredGrantAcceptance,
    launch: &SponsoredLaunchAuthority,
) -> Result<(), SponsorshipError> {
    acceptance.validate()?;
    if acceptance.launch_id != launch.launch_id
        || acceptance.contributor != launch.contributor
        || acceptance.revision_id != launch.revision_id
        || acceptance.revision_digest != launch.revision_digest
        || acceptance.policy_id != launch.policy_id
        || acceptance.launch_snapshot_digest != launch.launch_snapshot_digest
    {
        return Err(SponsorshipError::TermsNotAccepted);
    }
    Ok(())
}

pub(super) fn require_grant_bindings(
    grant: &SponsoredGrant,
    launch: &SponsoredLaunchAuthority,
) -> Result<(), SponsorshipError> {
    if grant.tenant_id != launch.tenant_id
        || grant.launch_id != launch.launch_id
        || launch.session_id.as_ref() != Some(&grant.session_id)
        || grant.contributor != launch.contributor
        || grant.trigger_revision_id != launch.revision_id
        || grant.trigger_revision_digest != launch.revision_digest
        || grant.policy_id != launch.policy_id
    {
        return Err(SponsorshipError::ScopeDenied);
    }
    Ok(())
}

pub(super) fn require_stored_grant_policy_bindings(
    grant: &SponsoredGrant,
    policy: &SponsorshipPolicy,
) -> Result<(), SponsorshipStoreError> {
    grant
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    policy
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if grant.policy_id != policy.id
        || grant.tenant_id != policy.tenant_id
        || grant.sponsor_subject != *policy.owner.controller()
        || grant.policy_generation > policy.generation
    {
        return Err(SponsorshipStoreError::Malformed(
            "stored grant authority does not match its referenced sponsorship policy".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_provider_grant_receipt(
    receipt: &ProviderGrantReceipt,
    policy: &SponsorshipPolicy,
    now: &str,
) -> Result<(), SponsorshipError> {
    validate_identifier(
        "provider_grant.provider_grant_id",
        receipt
            .provider_grant_id
            .expose_for_provider_or_encryption(),
        512,
    )?;
    validate_identifier(
        "provider_grant.revocation_handle",
        receipt
            .revocation_handle
            .expose_for_provider_or_encryption(),
        512,
    )?;
    validate_timestamp("provider_grant.expires_at", &receipt.expires_at)?;
    validate_timestamp("provider_grant.issued_at", &receipt.issued_at)?;
    let now = crate::seconds_from_rfc3339(now)
        .ok_or_else(|| TriggerValidationError::field("now", "must be a CMA timestamp"))?;
    let expires = crate::seconds_from_rfc3339(&receipt.expires_at).ok_or_else(|| {
        TriggerValidationError::field("provider_grant.expires_at", "must be a CMA timestamp")
    })?;
    let policy_expires = crate::seconds_from_rfc3339(&policy.expires_at).ok_or_else(|| {
        TriggerValidationError::field("policy.expires_at", "must be a CMA timestamp")
    })?;
    let issued = crate::seconds_from_rfc3339(&receipt.issued_at).ok_or_else(|| {
        TriggerValidationError::field("provider_grant.issued_at", "must be a CMA timestamp")
    })?;
    if receipt.provider_state_version == 0
        || issued > now
        || expires <= now
        || expires > policy_expires
    {
        return Err(SponsorshipError::ProviderRejected);
    }
    Ok(())
}

pub(super) fn require_receipt_matches_grant(
    operational: &SponsorshipOperationalPseudonymizer,
    receipt: &ProviderGrantReceipt,
    grant: &SponsoredGrant,
) -> Result<(), SponsorshipError> {
    let grant_id_candidates = operational.pseudonymize(
        SponsorshipOperationalDomain::ProviderGrantAuthority,
        &[
            &grant.id.0,
            receipt
                .provider_grant_id
                .expose_for_provider_or_encryption(),
        ],
    )?;
    let revocation_candidates = operational.pseudonymize(
        SponsorshipOperationalDomain::ProviderRevocationAuthority,
        &[
            &grant.id.0,
            receipt
                .revocation_handle
                .expose_for_provider_or_encryption(),
        ],
    )?;
    if !grant_id_candidates
        .iter()
        .any(|candidate| candidate.as_str() == grant.authority.provider_grant_id_digest)
        || !revocation_candidates
            .iter()
            .any(|candidate| candidate.as_str() == grant.authority.revocation_handle_digest)
        || grant.authority.provider_state_version != receipt.provider_state_version
        || grant.authority.expires_at != receipt.expires_at
        || grant.authority.issued_at != receipt.issued_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "encrypted provider receipt does not match the public grant authority".into(),
        )
        .into());
    }
    Ok(())
}

pub(super) fn activation_fingerprint(
    launch: &SponsoredLaunchAuthority,
    acceptance: &SponsoredGrantAcceptance,
    gate_fence: &SponsorshipGateFence,
    allocation: &SponsoredUsageSummary,
    policy_generation: u64,
    credential_generation: u64,
) -> Result<String, TriggerValidationError> {
    canonical_digest(&(
        "cma-sponsored-activation-fingerprint-v1",
        &launch.digest,
        &acceptance.digest,
        &gate_fence.configuration_digest,
        gate_fence.epoch,
        allocation,
        policy_generation,
        credential_generation,
    ))
}

pub(super) fn credential_operation_id(
    kind: ProviderOperationKind,
    credential_id: &SponsoredCredentialId,
    generation: u64,
    payload_digest: &str,
) -> Result<String, TriggerValidationError> {
    canonical_digest(&(
        "cma-sponsored-credential-operation-v1",
        provider_operation_kind_name(kind),
        credential_id,
        generation,
        payload_digest,
    ))
}

pub(super) fn grant_operation_id(
    kind: ProviderOperationKind,
    grant_id: &SponsoredGrantId,
    generation: u64,
    payload_digest: &str,
) -> Result<String, TriggerValidationError> {
    canonical_digest(&(
        "cma-sponsored-grant-operation-v1",
        provider_operation_kind_name(kind),
        grant_id,
        generation,
        payload_digest,
    ))
}

pub(super) fn provider_operation_kind_name(kind: ProviderOperationKind) -> &'static str {
    match kind {
        ProviderOperationKind::RotateCredential => "rotate_credential",
        ProviderOperationKind::RevokeCredential => "revoke_credential",
        ProviderOperationKind::RevokeGrant => "revoke_grant",
        ProviderOperationKind::CloseGrant => "close_grant",
        ProviderOperationKind::RevokeUncommittedGrant => "revoke_uncommitted_grant",
    }
}

pub(super) fn deterministic_grant_id(
    provider_request_id: &str,
) -> Result<SponsoredGrantId, TriggerValidationError> {
    SponsorshipOperationalPseudonym::parse(provider_request_id.to_string())?;
    let digest = Sha256::digest(
        [
            b"cma-sponsored-grant-id-v1:".as_slice(),
            provider_request_id.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Mark this as an RFC 4122 variant/version-5-shaped deterministic UUID.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(SponsoredGrantId(Uuid::from_bytes(bytes).to_string()))
}

pub(super) fn deterministic_credential_id(
    authority: &ProviderCredentialAuthority,
) -> Result<SponsoredCredentialId, TriggerValidationError> {
    validate_policy_token("credential.provider", &authority.provider)?;
    SponsorshipOperationalPseudonym::parse(authority.provider_credential_id_digest.clone())?;
    let identity = canonical_digest(&(
        "cma-sponsored-credential-identity-v1",
        &authority.provider,
        &authority.provider_credential_id_digest,
    ))?;
    let digest = Sha256::digest(
        [
            b"cma-sponsored-credential-id-v1:".as_slice(),
            identity.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(SponsoredCredentialId(Uuid::from_bytes(bytes).to_string()))
}

pub(super) fn map_provider_error(error: SponsoredProviderError) -> SponsorshipError {
    match error {
        SponsoredProviderError::Ambiguous => SponsorshipError::ProviderAmbiguous,
        SponsoredProviderError::Unavailable => SponsorshipError::ProviderUnavailable,
        SponsoredProviderError::Rejected | SponsoredProviderError::MalformedEvidence => {
            SponsorshipError::ProviderRejected
        }
    }
}

pub(super) fn map_pricing_error(error: SponsoredPricingError) -> SponsorshipError {
    match error {
        SponsoredPricingError::Unavailable => SponsorshipError::PricingUnavailable,
        SponsoredPricingError::Unsupported | SponsoredPricingError::Malformed => {
            SponsorshipError::PricingUnsupported
        }
    }
}

pub(super) fn next_generation(current: u64, field: &str) -> Result<u64, TriggerValidationError> {
    current
        .checked_add(1)
        .ok_or_else(|| TriggerValidationError::field(field, "is exhausted"))
}

pub(super) fn validate_stable_request_id(value: &str) -> Result<(), TriggerValidationError> {
    if value.len() < 8
        || value.len() > 255
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err(TriggerValidationError::field(
            "stable_request_id",
            "must be an 8-255 byte opaque identifier",
        ));
    }
    Ok(())
}

pub(super) fn validate_policy_token(
    field: &str,
    value: &str,
) -> Result<(), TriggerValidationError> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
    {
        return Err(TriggerValidationError::field(
            field,
            "must be a bounded canonical policy token",
        ));
    }
    Ok(())
}

pub(super) fn validate_identifier(
    field: &str,
    value: &str,
    maximum: usize,
) -> Result<(), TriggerValidationError> {
    if value.trim().is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(TriggerValidationError::field(
            field,
            format!("must contain 1-{maximum} printable bytes"),
        ));
    }
    Ok(())
}

pub(super) fn validate_display_name(
    field: &str,
    value: &str,
) -> Result<(), TriggerValidationError> {
    validate_identifier(field, value, 160)
}

pub(super) fn validate_digest(field: &str, value: &str) -> Result<(), TriggerValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TriggerValidationError::field(
            field,
            "must be 64 lowercase hexadecimal characters",
        ));
    }
    Ok(())
}

pub(super) fn validate_timestamp(field: &str, value: &str) -> Result<(), TriggerValidationError> {
    if crate::seconds_from_rfc3339(value).is_none() {
        return Err(TriggerValidationError::field(
            field,
            "must be a CMA timestamp",
        ));
    }
    Ok(())
}

pub(super) fn canonical_digest<T: Serialize + ?Sized>(
    value: &T,
) -> Result<String, TriggerValidationError> {
    serde_json::to_vec(value)
        .map(|bytes| lower_hex(Sha256::digest(bytes)))
        .map_err(|_| TriggerValidationError::new("canonical sponsorship data cannot be encoded"))
}

pub(super) fn lower_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
