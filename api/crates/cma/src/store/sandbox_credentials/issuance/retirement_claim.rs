//! Independent discovery and leases for a known late retirement result.
use super::*;
use futures_util::TryStreamExt;
use layer2_core::sandbox::SandboxCredentialAuthority;
use mongodb::bson::DateTime;
use mongodb::options::{Hint, ReturnDocument};

const LEASE_SECONDS: i64 = 90;
const RETRY_SECONDS: i64 = 30;
const DISCOVERY_LIMIT: i64 = 16;

// Provider dispatch and its account-erasure admission remain in the next
// draft integration. These leases only establish durable custody ownership.
#[allow(
    dead_code,
    reason = "independent revoker awaits provider and erasure qualification"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetirementRevocationClaim {
    pub issuance: CredentialIssuance,
    pub owner_authority: RetirementOwnerAuthority,
    pub provider_authority: SandboxCredentialAuthority,
    pub holder: String,
    pub token: String,
    pub expires_at: DateTime,
}

pub(super) fn marker(row: &Document) -> Result<&Document, CredentialCustodyError> {
    marker_for_state(row, "pending")
}

pub(super) fn marker_for_state<'a>(
    row: &'a Document,
    state: &str,
) -> Result<&'a Document, CredentialCustodyError> {
    let marker = row
        .get_document("retirement_custody")
        .map_err(|_| corrupt())?;
    if marker.get_i64("version").ok() != Some(1)
        || marker.get_str("state").ok() != Some(state)
        || marker.get_str("reason").ok() != Some("initial_issuance_unresolved")
    {
        return Err(corrupt());
    }
    Ok(marker)
}

fn frozen(intent: &CredentialIssuance) -> Result<Document, CredentialCustodyError> {
    let mut identity = mongodb::bson::serialize_to_document(intent).map_err(|_| corrupt())?;
    identity.remove("next_check_at");
    Ok(identity)
}

fn authority<T: serde::de::DeserializeOwned>(
    row: &Document,
    name: &str,
) -> Result<T, CredentialCustodyError> {
    mongodb::bson::deserialize_from_bson(row.get(name).cloned().ok_or_else(corrupt)?)
        .map_err(|_| corrupt())
}

pub(super) fn work_for_state(
    row: &Document,
    state: &str,
) -> Result<
    (
        CredentialIssuance,
        RetirementOwnerAuthority,
        SandboxCredentialAuthority,
    ),
    CredentialCustodyError,
> {
    let intent = decode(row.clone())?;
    let marker = marker_for_state(row, state)?;
    let owner: RetirementOwnerAuthority = authority(marker, "owner_authority")?;
    let provider: SandboxCredentialAuthority = authority(marker, "late_provider_authority")?;
    if intent.generation != 1
        || intent.state != IssuanceState::Unknown
        || !matches!(intent.identity, IssuanceIdentity::Prepared { .. })
        || marker.get_document("issuance").ok() != Some(&frozen(&intent)?)
        || marker.get("owner_authority") != row.get("issuance_owner_authority")
        || owner.owner() != &intent.owner
        || !super::retirement::valid_owner_authority(&owner)
        || !valid_provider_key_id(&provider.provider_key_id)
        || provider.generation != intent.generation
        || provider.issued_at != intent.created_at
        || provider.provider_state_version <= 0
        || provider.grant_fingerprint.len() != 64
        || !provider
            .grant_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || seconds_from_rfc3339(&provider.issued_at)
            .zip(seconds_from_rfc3339(&provider.expires_at))
            .is_none_or(|(issued, expires)| expires <= issued)
    {
        return Err(corrupt());
    }
    Ok((intent, owner, provider))
}

fn exact(
    intent: &CredentialIssuance,
    owner: &RetirementOwnerAuthority,
    provider: &SandboxCredentialAuthority,
) -> Result<Document, CredentialCustodyError> {
    let frozen = frozen(intent)?;
    let selector = mongodb::bson::serialize_to_bson(owner).map_err(|_| corrupt())?;
    let mut filter = doc! {"_id":intent.sandbox_id.as_str(), "document_schema_version":1_i64,
    "retirement_custody.version":1_i64, "retirement_custody.state":"pending",
    "retirement_custody.reason":"initial_issuance_unresolved",
    "retirement_custody.issuance": &frozen,
    "issuance_owner_authority": &selector, "retirement_custody.owner_authority": selector,
    "retirement_custody.late_provider_authority": mongodb::bson::serialize_to_bson(provider).map_err(|_| corrupt())?,
    "retirement_custody.revocation_observation":{"$exists":false}};
    for (field, value) in frozen {
        filter.insert(format!("record.{field}"), value);
    }
    Ok(filter)
}

fn eligible() -> Document {
    doc! {"document_schema_version":1_i64, "record.state":"unknown",
    "retirement_custody.version":1_i64, "retirement_custody.state":"pending",
    "retirement_custody.owner_authority":{"$exists":true},
    "retirement_custody.late_provider_authority":{"$exists":true},
    "retirement_custody.revocation_observation":{"$exists":false},
    "$and":[
        {"$or":[{"retirement_custody.claim":{"$exists":false}},
            {"$expr":{"$and":[{"$eq":[{"$type":"$retirement_custody.claim.expires_at"},"date"]},
                {"$lte":["$retirement_custody.claim.expires_at","$$NOW"]}]}}]},
        {"$or":[{"retirement_discovery_at":{"$exists":false}},
            {"$expr":{"$and":[{"$eq":[{"$type":"$retirement_discovery_at"},"date"]},
                {"$lte":["$retirement_discovery_at","$$NOW"]}]}}]},
        {"$or":[{"retirement_discovery_refusal.retry_at":{"$exists":false}},
            {"$expr":{"$and":[{"$eq":[{"$type":"$retirement_discovery_refusal.retry_at"},"date"]},
                {"$lte":["$retirement_discovery_refusal.retry_at","$$NOW"]}]}}]},
        {"$or":[{"retirement_custody.retry_at":{"$exists":false}},
            {"$expr":{"$and":[{"$eq":[{"$type":"$retirement_custody.retry_at"},"date"]},
                {"$lte":["$retirement_custody.retry_at","$$NOW"]}]}}]},
    ]}
}

// A v1 marker predating indexed scheduling might already have a lease or a
// retry deadline. Retain the latest of those dates when first indexing it.
fn legacy_schedule(row: &Document) -> Result<Option<DateTime>, CredentialCustodyError> {
    fn optional_date(
        doc: &Document,
        field: &str,
    ) -> Result<Option<DateTime>, CredentialCustodyError> {
        match doc.get(field) {
            None => Ok(None),
            Some(Bson::DateTime(date)) => Ok(Some(*date)),
            Some(_) => Err(corrupt()),
        }
    }
    let scheduled = optional_date(row, "retirement_discovery_at")?;
    let marker = marker(row)?;
    if marker.contains_key("revocation_observation") {
        return Err(corrupt());
    }
    let retry = optional_date(marker, "retry_at")?;
    let lease = marker
        .get("claim")
        .map(|value| {
            let claim = value.as_document().ok_or_else(corrupt)?;
            claim
                .get_datetime("expires_at")
                .copied()
                .map_err(|_| corrupt())
        })
        .transpose()?;
    let refusal = row
        .get("retirement_discovery_refusal")
        .map(|value| {
            let refusal = value.as_document().ok_or_else(corrupt)?;
            refusal
                .get_datetime("retry_at")
                .copied()
                .map_err(|_| corrupt())
        })
        .transpose()?;
    Ok(retry
        .max(lease)
        .max(refusal)
        .filter(|due| scheduled.is_none_or(|at| *due > at)))
}

pub(super) fn observed(row: &Document) -> Result<Document, CredentialCustodyError> {
    let id = row.get("_id").cloned().ok_or_else(corrupt)?;
    Ok(doc! {"_id":{"$eq":id}, "$expr":{"$eq":["$$ROOT",{"$literal":row}]}})
}

fn after_seconds(seconds: i64) -> Document {
    doc! {"$dateAdd":{"startDate":"$$NOW","unit":"second","amount":seconds}}
}

fn refusal_deadline() -> Document {
    let mut dates = vec![Bson::Document(after_seconds(RETRY_SECONDS))];
    for field in [
        "$retirement_custody.claim.expires_at",
        "$retirement_custody.retry_at",
        "$retirement_discovery_refusal.retry_at",
    ] {
        dates.push(Bson::Document(
            doc! {"$cond":[{"$eq":[{"$type":field},"date"]},field,"$$NOW"]},
        ));
    }
    doc! {"$max":dates}
}

pub(super) fn current(
    claim: &RetirementRevocationClaim,
) -> Result<Document, CredentialCustodyError> {
    let mut filter = exact(
        &claim.issuance,
        &claim.owner_authority,
        &claim.provider_authority,
    )?;
    filter.insert("retirement_custody.claim.token", &claim.token);
    filter.insert("retirement_custody.claim.holder", &claim.holder);
    filter.insert(
        "$expr",
        doc! {"$and":[
        {"$eq":[{"$type":"$retirement_custody.claim.expires_at"},"date"]},
        {"$gt":["$retirement_custody.claim.expires_at","$$NOW"]}]},
    );
    Ok(filter)
}

#[allow(
    dead_code,
    reason = "independent revoker awaits provider and erasure qualification"
)]
impl MongoSandboxCredentialStore {
    /// Bounded discovery from the custody collection, independent of Sandbox
    /// existence and its lifecycle work claim. No provider effect is admitted.
    pub(crate) async fn claim_retirement_revocation(
        &self,
        holder: &str,
    ) -> Result<Option<RetirementRevocationClaim>, CredentialCustodyError> {
        if holder.trim().is_empty() || holder.len() > 128 || holder.chars().any(char::is_control) {
            return Err(corrupt());
        }
        // The partial index uses this exact candidate predicate and sort.
        // Eligibility is checked after the bounded fetch, under the claim CAS.
        let rows: Vec<Document> = self
            .intents
            .find(retirement_discovery_candidates())
            .hint(Hint::Name(RETIREMENT_DISCOVERY_INDEX.into()))
            .sort(doc! {"retirement_discovery_at":1, "_id":1})
            .limit(DISCOVERY_LIMIT)
            .await
            .map_err(unavailable)?
            .try_collect()
            .await
            .map_err(unavailable)?;
        for row in rows {
            let checked =
                work_for_state(&row, "pending").and_then(|work| Ok((work, legacy_schedule(&row)?)));
            let ((issuance, owner_authority, provider_authority), legacy_due) = match checked {
                Ok(checked) => checked,
                Err(CredentialCustodyError::Corrupt(_)) => {
                    // Preserve the unresolved obligation, even if its marker
                    // has an invalid shape. A concurrent repair wins the CAS.
                    self.intents
                        .update_one(
                            doc! {"$and":[observed(&row)?, {"$expr":{"$or":[
                                {"$ne":[{"$type":"$retirement_discovery_at"},"date"]},
                                {"$lte":["$retirement_discovery_at","$$NOW"]}
                            ]}}]},
                            vec![doc! {"$set":{
                                "retirement_discovery_at": refusal_deadline(),
                                "retirement_discovery_refusal":{
                                    "reason":"invalid_retirement_identity",
                                    "observed_at":"$$NOW",
                                    "retry_at": refusal_deadline()
                                }
                            }}],
                        )
                        .await
                        .map_err(unavailable)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(due) = legacy_due {
                let normalized = self
                    .intents
                    .update_one(
                        observed(&row)?,
                        doc! {"$set":{"retirement_discovery_at":due}},
                    )
                    .await
                    .map_err(unavailable)?;
                if normalized.matched_count != 1 {
                    continue;
                }
            }
            let token = uuid::Uuid::new_v4().simple().to_string();
            let filter = doc! {"$and":[eligible(), exact(&issuance, &owner_authority, &provider_authority)?]};
            let Some(claimed) = self.intents.find_one_and_update(filter, vec![doc! {"$set": {
                "retirement_custody.claim": {"holder":{"$literal":holder}, "token":{"$literal":&token},
                    "expires_at":after_seconds(LEASE_SECONDS)},
                "retirement_discovery_at":after_seconds(LEASE_SECONDS)
            }}]).return_document(ReturnDocument::After).await.map_err(unavailable)? else { continue; };
            let (read_issuance, read_owner, read_provider) = work_for_state(&claimed, "pending")?;
            let lease = marker(&claimed)?
                .get_document("claim")
                .map_err(|_| corrupt())?;
            if frozen(&read_issuance)? != frozen(&issuance)?
                || read_owner != owner_authority
                || read_provider != provider_authority
                || lease.get_str("holder").ok() != Some(holder)
                || lease.get_str("token").ok() != Some(token.as_str())
            {
                return Err(corrupt());
            }
            return Ok(Some(RetirementRevocationClaim {
                issuance,
                owner_authority,
                provider_authority,
                holder: holder.to_owned(),
                token,
                expires_at: *lease.get_datetime("expires_at").map_err(|_| corrupt())?,
            }));
        }
        Ok(None)
    }

    pub(crate) async fn retirement_revocation_claim_current(
        &self,
        claim: &RetirementRevocationClaim,
    ) -> Result<bool, CredentialCustodyError> {
        self.intents
            .find_one(current(claim)?)
            .projection(doc! {"_id":1})
            .await
            .map(|row| row.is_some())
            .map_err(unavailable)
    }

    pub(crate) async fn defer_retirement_revocation(
        &self,
        claim: &RetirementRevocationClaim,
    ) -> Result<bool, CredentialCustodyError> {
        let updated = self
            .intents
            .update_one(
                current(claim)?,
                vec![
                    doc! {"$set":{"retirement_custody.retry_at":after_seconds(RETRY_SECONDS),
                    "retirement_discovery_at":after_seconds(RETRY_SECONDS)}},
                    doc! {"$unset":"retirement_custody.claim"},
                ],
            )
            .await
            .map_err(unavailable)?;
        Ok(updated.matched_count == 1)
    }
}
