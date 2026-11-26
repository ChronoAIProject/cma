//! Fence a known-key retirement revocation against account erasure.
use super::retirement_claim::{current, marker, observed, RetirementRevocationClaim};
use super::*;
use crate::auth::keys::ConfirmedRetirementKeyRevocation;

fn dispatch_identity(
    claim: &RetirementRevocationClaim,
    provider_identity: &str,
) -> Result<Document, CredentialCustodyError> {
    let IssuanceIdentity::Prepared {
        provider_identity: original,
        ..
    } = &claim.issuance.identity
    else {
        return Err(corrupt());
    };
    if provider_identity != original || claim.issuance.owner != *claim.owner_authority.owner() {
        return Err(corrupt());
    }
    Ok(doc! {
        "operation_id": &claim.issuance.operation_id,
        "provider_identity": original,
        "provider_key_id": &claim.provider_authority.provider_key_id,
    })
}

/// Validate the persisted known-key evidence on subsequent retirement and
/// late-result replays. The original issuance identity remains the authority.
pub(super) fn validate_key_revocation(
    marker: &Document,
    intent: &CredentialIssuance,
) -> Result<(), CredentialCustodyError> {
    let IssuanceIdentity::Prepared {
        provider_identity, ..
    } = &intent.identity
    else {
        return Err(corrupt());
    };
    let provider = marker
        .get_document("late_provider_authority")
        .map_err(|_| corrupt())?;
    let key = provider.get_str("provider_key_id").map_err(|_| corrupt())?;
    let identity = doc! {"operation_id":&intent.operation_id,"provider_identity":provider_identity,"provider_key_id":key};
    let dispatch = marker
        .get_document("revocation_dispatch")
        .map_err(|_| corrupt())?;
    let observation = marker
        .get_document("revocation_observation")
        .map_err(|_| corrupt())?;
    if !valid_provider_key_id(key)
        || dispatch.get_document("identity").ok() != Some(&identity)
        || observation.get_document("identity").ok() != Some(&identity)
        || observation.get_i64("http_status").ok() != Some(200)
        || dispatch
            .get_str("last_claim_token")
            .map_err(|_| corrupt())?
            != observation.get_str("claim_token").map_err(|_| corrupt())?
        || observation.get_datetime("observed_at").is_err()
    {
        return Err(corrupt());
    }
    Ok(())
}

/// A committed admission is specific to one claim and one original provider
/// operation. Its durable record is preserved even if the caller loses its
/// acknowledgement or the external effect becomes ambiguous.
#[allow(
    dead_code,
    reason = "independent dispatch awaits provider qualification"
)]
#[derive(Debug)]
pub(crate) struct RetirementRevocationAdmission {
    claim: RetirementRevocationClaim,
    identity: Document,
}

#[allow(
    dead_code,
    reason = "independent dispatch awaits provider qualification"
)]
impl MongoSandboxCredentialStore {
    pub(crate) async fn admit_retirement_revocation(
        &self,
        claim: &RetirementRevocationClaim,
        provider_identity: &str,
    ) -> Result<Option<RetirementRevocationAdmission>, CredentialCustodyError> {
        let identity = dispatch_identity(claim, provider_identity)?;
        let database = self.client.database(&self.intents.namespace().db);
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let outcome = async {
            // Taking this write fence in the same transaction as the durable
            // disposition serializes admission with account erasure.
            if crate::store::account_deletions::fence_subject_in_session(
                &database,
                &claim.issuance.owner,
                &mut session,
            )
            .await
            .map_err(unavailable)?
            {
                return Ok(None);
            }
            let Some(row) = self
                .intents
                .find_one(current(claim)?)
                .session(&mut session)
                .await
                .map_err(unavailable)?
            else {
                return Ok(None);
            };
            let custody = marker(&row)?;
            let mut first = Bson::String("$$NOW".into());
            let mut attempts = 1_i64;
            if let Some(previous) = custody.get("revocation_dispatch") {
                let previous = previous.as_document().ok_or_else(corrupt)?;
                if previous.get_document("identity").ok() != Some(&identity) {
                    return Err(corrupt());
                }
                if previous
                    .get_str("last_claim_token")
                    .map_err(|_| corrupt())?
                    == claim.token
                {
                    // A replay of this admission cannot authorize a second
                    // effect from the same claim. Only a successor claim may
                    // try the same immutable target again.
                    return Ok(None);
                }
                first = Bson::DateTime(
                    *previous
                        .get_datetime("first_admitted_at")
                        .map_err(|_| corrupt())?,
                );
                previous
                    .get_datetime("last_admitted_at")
                    .map_err(|_| corrupt())?;
                let previous_attempts = previous.get_i64("attempts").map_err(|_| corrupt())?;
                if previous_attempts < 1 {
                    return Err(corrupt());
                }
                attempts = previous_attempts.checked_add(1).ok_or_else(corrupt)?;
            }
            let updated = self
                .intents
                .update_one(
                    doc! {"$and":[current(claim)?, observed(&row)?]},
                    vec![doc! {"$set": {"retirement_custody.revocation_dispatch": {
                        "identity":{"$literal":&identity},
                        "first_admitted_at":first,
                        "last_admitted_at":"$$NOW",
                        "last_claim_token":{"$literal":&claim.token},
                        "attempts":attempts,
                    }}}],
                )
                .session(&mut session)
                .await
                .map_err(unavailable)?;
            if updated.matched_count != 1 {
                return Ok(None);
            }
            Ok(Some(RetirementRevocationAdmission {
                claim: claim.clone(),
                identity,
            }))
        }
        .await;
        transactions::finish(&mut session, outcome).await
    }

    pub(crate) async fn retirement_revocation_admission_current(
        &self,
        admission: &RetirementRevocationAdmission,
    ) -> Result<bool, CredentialCustodyError> {
        self.intents
            .find_one(doc! {"$and":[current(&admission.claim)?, {
                "retirement_custody.revocation_dispatch.identity":&admission.identity,
                "retirement_custody.revocation_dispatch.last_claim_token":&admission.claim.token,
            }]})
            .projection(doc! {"_id":1})
            .await
            .map(|row| row.is_some())
            .map_err(unavailable)
    }
    /// Record only affirmative exact-key revocation under the current claim.
    /// The original issuance remains Unknown until retirement consumes an
    /// independently qualified settlement for its complete obligation.
    pub(crate) async fn record_retirement_key_revocation(
        &self,
        admission: &RetirementRevocationAdmission,
        evidence: &ConfirmedRetirementKeyRevocation,
    ) -> Result<bool, CredentialCustodyError> {
        let provider = admission
            .identity
            .get_str("provider_identity")
            .map_err(|_| corrupt())?;
        let key = admission
            .identity
            .get_str("provider_key_id")
            .map_err(|_| corrupt())?;
        if !evidence.matches(provider, key) {
            return Err(corrupt());
        }
        let updated = self.intents.update_one(doc! {"$and":[current(&admission.claim)?, {
            "retirement_custody.revocation_dispatch.identity": &admission.identity,
            "retirement_custody.revocation_dispatch.last_claim_token": &admission.claim.token,
        }]}, vec![doc! {"$set": {
            "retirement_custody.state":"key_revoked",
            "retirement_custody.revocation_observation":{
                "identity":{"$literal":&admission.identity},
                "claim_token":{"$literal":&admission.claim.token},
                "http_status":200_i64,
                "observed_at":"$$NOW",
            }
        }}]).await.map_err(unavailable)?;
        Ok(updated.matched_count == 1)
    }
}
