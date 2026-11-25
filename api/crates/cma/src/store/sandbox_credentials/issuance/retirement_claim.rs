//! Independent discovery and leases for a known late retirement result.
use super::*;
use layer2_core::sandbox::SandboxCredentialAuthority;
use mongodb::bson::DateTime;
use mongodb::options::ReturnDocument;

const LEASE_SECONDS: i64 = 90;
const RETRY_SECONDS: i64 = 30;

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

fn marker(row: &Document) -> Result<&Document, CredentialCustodyError> {
    let marker = row
        .get_document("retirement_custody")
        .map_err(|_| corrupt())?;
    if marker.get_i64("version").ok() != Some(1)
        || marker.get_str("state").ok() != Some("pending")
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

fn work(
    row: &Document,
) -> Result<
    (
        CredentialIssuance,
        RetirementOwnerAuthority,
        SandboxCredentialAuthority,
    ),
    CredentialCustodyError,
> {
    let intent = decode(row.clone())?;
    let marker = marker(row)?;
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
        {"$or":[{"retirement_custody.retry_at":{"$exists":false}},
            {"$expr":{"$and":[{"$eq":[{"$type":"$retirement_custody.retry_at"},"date"]},
                {"$lte":["$retirement_custody.retry_at","$$NOW"]}]}}]},
    ]}
}

fn current(claim: &RetirementRevocationClaim) -> Result<Document, CredentialCustodyError> {
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
        let Some(row) = self
            .intents
            .find_one(eligible())
            .sort(doc! {"retirement_custody.retry_at":1, "_id":1})
            .await
            .map_err(unavailable)?
        else {
            return Ok(None);
        };
        let (issuance, owner_authority, provider_authority) = work(&row)?;
        let token = uuid::Uuid::new_v4().simple().to_string();
        let filter =
            doc! {"$and":[eligible(), exact(&issuance, &owner_authority, &provider_authority)?]};
        let Some(claimed) = self.intents.find_one_and_update(filter, vec![doc! {"$set": {
            "retirement_custody.claim": {"holder":{"$literal":holder}, "token":{"$literal":&token},
                "expires_at":{"$dateAdd":{"startDate":"$$NOW", "unit":"second", "amount":LEASE_SECONDS}}}
        }}]).return_document(ReturnDocument::After).await.map_err(unavailable)? else { return Ok(None); };
        let (read_issuance, read_owner, read_provider) = work(&claimed)?;
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
        Ok(Some(RetirementRevocationClaim {
            issuance,
            owner_authority,
            provider_authority,
            holder: holder.to_owned(),
            token,
            expires_at: *lease.get_datetime("expires_at").map_err(|_| corrupt())?,
        }))
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
        let updated = self.intents.update_one(current(claim)?, vec![
            doc! {"$set":{"retirement_custody.retry_at":{"$dateAdd":{"startDate":"$$NOW","unit":"second","amount":RETRY_SECONDS}}}},
            doc! {"$unset":"retirement_custody.claim"},
        ]).await.map_err(unavailable)?;
        Ok(updated.matched_count == 1)
    }
}
