//! Validated persistence representation for credential issuance obligations.
use crate::sandbox::{CredentialCustodyError, CredentialIssuance, IssuanceIdentity, IssuanceState};
use layer2_core::seconds_from_rfc3339;
use mongodb::bson::Document;

pub(super) fn decode(row: Document) -> Result<CredentialIssuance, CredentialCustodyError> {
    if row.get_i64("document_schema_version").ok() != Some(1) {
        return Err(corrupt());
    }
    let intent: CredentialIssuance =
        mongodb::bson::deserialize_from_bson(row.get("record").cloned().ok_or_else(corrupt)?)
            .map_err(|_| corrupt())?;
    validate(&intent)?;
    if row.get_str("_id").ok() != Some(intent.sandbox_id.as_str()) {
        return Err(corrupt());
    }
    Ok(intent)
}

pub(super) fn validate(intent: &CredentialIssuance) -> Result<(), CredentialCustodyError> {
    let digest = |value: &str| {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    if intent
        .operation_id
        .strip_prefix("mint_")
        .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
        || intent.generation == 0
        || intent.original_holder.is_empty()
        || intent.original_fence == 0
        || match &intent.identity {
            IssuanceIdentity::Prepared {
                provider_identity,
                request_digest,
            } => !digest(provider_identity) || !digest(request_digest),
            IssuanceIdentity::LegacyUnobserved => {
                intent.state != IssuanceState::Unknown || intent.owner_authority.is_some()
            }
        }
        || intent
            .owner_authority
            .is_some_and(|version| version.granted_at_ms == 0)
        || seconds_from_rfc3339(&intent.created_at).is_none()
        || seconds_from_rfc3339(&intent.next_check_at).is_none()
    {
        return Err(corrupt());
    }
    Ok(())
}

fn corrupt() -> CredentialCustodyError {
    CredentialCustodyError::Corrupt("credential issuance custody refused".into())
}
